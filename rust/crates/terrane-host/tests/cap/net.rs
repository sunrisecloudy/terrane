//! e2e smoke for `net`.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use tempfile::tempdir;

use crate::helpers::terrane;

fn loopback_server<F>(requests: usize, handler: F) -> (String, thread::JoinHandle<()>)
where
    F: Fn(String, TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let handler = std::sync::Arc::new(handler);
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let mut buf = [0; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            handler(request, stream);
        }
    });
    (addr, handle)
}

fn respond(mut stream: TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n",
        body.len()
    )
    .unwrap();
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").unwrap();
    }
    stream.write_all(b"\r\n").unwrap();
    stream.write_all(body).unwrap();
}

#[test]
fn net_request_posts_redacts_and_replays_on_loopback() {
    let (base, server) = loopback_server(1, |request, stream| {
        assert!(request.starts_with("POST /items?token=query HTTP/1.1"), "{request}");
        assert!(request.contains("authorization: Bearer raw-secret"), "{request}");
        assert!(request.ends_with("{\"ok\":true}"), "{request}");
        respond(
            stream,
            "201 Created",
            &[("Content-Type", "application/json"), ("X-Secret", "leak")],
            br#"{"saved":true}"#,
        );
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);
    let request = format!(
        r#"{{
            "method":"POST",
            "url":"{base}/items?token=query",
            "headers":{{"Authorization":"Bearer raw-secret","X-Trace":"ok"}},
            "body":"{{\"ok\":true}}",
            "responseBody":"inline"
        }}"#
    );

    let (ok, out, err) = terrane(home, &["net", "request", "web", &request]);
    assert!(ok, "request failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("net.responded"), "out: {out}");
    server.join().unwrap();

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(log.contains("net.responded web POST 127.0.0.1:"));
    assert!(!log.contains("raw-secret"), "log leaked secret: {log}");
    assert!(!log.contains("token=query"), "log leaked query string: {log}");
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed; stdout: {out}; stderr: {err}");
}

#[test]
fn net_request_redirect_policies_are_enforced_on_loopback() {
    let (base, server) = loopback_server(4, |request, stream| {
        if request.starts_with("GET /redirect ") {
            respond(stream, "302 Found", &[("Location", "/final")], b"");
        } else if request.starts_with("GET /final ") {
            respond(stream, "200 OK", &[("Content-Type", "text/plain")], b"done");
        } else {
            respond(stream, "404 Not Found", &[], b"missing");
        }
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let follow = format!(r#"{{"url":"{base}/redirect","redirect":"follow"}}"#);
    let (ok, out, err) = terrane(home, &["net", "request", "web", &follow]);
    assert!(ok, "follow failed; stdout: {out}; stderr: {err}");

    let manual = format!(r#"{{"url":"{base}/redirect","redirect":"manual"}}"#);
    let (ok, out, err) = terrane(home, &["net", "request", "web", &manual]);
    assert!(ok, "manual failed; stdout: {out}; stderr: {err}");

    let deny = format!(r#"{{"url":"{base}/redirect","redirect":"deny"}}"#);
    let (ok, _out, err) = terrane(home, &["net", "request", "web", &deny]);
    assert!(!ok, "deny should fail");
    assert!(err.contains("refused redirect status 302"), "{err}");
    server.join().unwrap();
}

#[test]
fn net_request_offloads_binary_response_to_blob() {
    let bytes = vec![0, 159, 146, 150, 255];
    let expected = B64.encode(&bytes);
    let (base, server) = loopback_server(1, move |_request, stream| {
        respond(
            stream,
            "200 OK",
            &[("Content-Type", "application/octet-stream")],
            &bytes,
        );
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);
    let request = format!(r#"{{"url":"{base}/bin","responseBody":"blob"}}"#);

    let (ok, out, err) = terrane(home, &["net", "request", "web", &request]);
    assert!(ok, "request failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("blob.stored"), "out: {out}");
    assert!(out.contains("net.responded"), "out: {out}");
    server.join().unwrap();

    let (ok, state, err) = terrane(home, &["state"]);
    assert!(ok, "state failed; stderr: {err}");
    assert!(state.contains("request "), "state: {state}");
    assert!(state.contains("-> 200 blob (5 bytes)"), "state: {state}");
    let (ok, blobs, err) = terrane(home, &["blob", "ls", "web", "__net__/"]);
    assert!(ok, "blob ls failed; stderr: {err}");
    assert!(blobs.contains("application/octet-stream"), "{blobs}");
    assert!(!blobs.contains(&expected), "blob metadata should not inline bytes: {blobs}");
}

#[test]
fn net_request_timeout_errors_on_loopback() {
    let (base, server) = loopback_server(1, |_request, mut stream| {
        thread::sleep(Duration::from_millis(300));
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);
    let request = format!(r#"{{"url":"{base}/slow","timeoutMs":50}}"#);

    let (ok, _out, err) = terrane(home, &["net", "request", "web", &request]);
    assert!(!ok, "timeout should fail");
    assert!(err.contains("failed") || err.contains("timed out"), "{err}");
    server.join().unwrap();
}

#[test]
#[ignore = "real network fetch; run with `cargo test -- --ignored`"]
fn net_e2e_smoke_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let (ok, out, err) = terrane(home, &["net", "fetch", "web", "http://example.com"]);
    assert!(ok, "fetch failed; stderr: {err}");
    assert!(out.contains("net.fetched"), "out: {out}");
}
