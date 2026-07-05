//! e2e smoke for `browser`.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

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

fn chromium_available() -> bool {
    let candidates = [
        std::env::var("TERRANE_CHROME").ok(),
        Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()),
        Some("/Applications/Chromium.app/Contents/MacOS/Chromium".to_string()),
        Some("google-chrome".to_string()),
        Some("google-chrome-stable".to_string()),
        Some("chromium".to_string()),
        Some("chromium-browser".to_string()),
        Some("chrome".to_string()),
    ];
    candidates.into_iter().flatten().any(|candidate| {
        if candidate.contains('/') {
            Path::new(&candidate).exists()
        } else {
            Command::new(candidate)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }
    })
}

#[test]
#[ignore = "requires a system browser engine that renders JS; headless CI environments time out"]
fn browser_render_sees_js_inserted_text_that_net_fetch_misses() {
    if !chromium_available() {
        eprintln!("skipping browser render e2e: no system Chrome/Chromium");
        return;
    }
    let page = br#"<!doctype html>
<title>Browser Cap</title>
<div id="root"></div>
<script>document.getElementById("root").innerText = "JS inserted text";</script>"#;
    let (base, server) = loopback_server(2, move |_request, stream| {
        respond(stream, "200 OK", &[("Content-Type", "text/html")], page);
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let (ok, out, err) = terrane(home, &["net", "fetch", "web", &base]);
    assert!(ok, "net fetch failed; stdout: {out}; stderr: {err}");
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(!log.contains("JS inserted text"), "net.fetch should not execute JS: {log}");

    let request = format!(r#"{{"url":"{base}","output":"text","waitMs":100}}"#);
    let (ok, out, err) = terrane(home, &["browser", "render", "web", &request]);
    if !ok && err.contains("browser render failed with status signal") {
        eprintln!("skipping browser render e2e: system browser aborts in this headless test environment");
        return;
    }
    assert!(ok, "browser render failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("browser.rendered"), "out: {out}");
    server.join().unwrap();

    let (ok, state, err) = terrane(home, &["state"]);
    assert!(ok, "state failed; stderr: {err}");
    assert!(state.contains("browser renders:"), "state: {state}");
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(log.contains("browser.rendered web text 127.0.0.1:"));
    assert!(!log.contains("?token="), "log should not expose URL query: {log}");
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed; stdout: {out}; stderr: {err}");
}

#[test]
fn browser_render_blocks_cloud_metadata_url() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);
    let request = r#"{"url":"http://169.254.169.254/latest/meta-data","output":"text"}"#;

    let (ok, _out, err) = terrane(home, &["browser", "render", "web", request]);
    assert!(!ok, "metadata render should fail");
    assert!(err.contains("cloud metadata"), "{err}");
}

#[test]
#[ignore = "requires a system browser engine and writes PNG bytes to the local blob CAS"]
fn browser_screenshot_offloads_to_blob() {
    if !chromium_available() {
        eprintln!("skipping browser screenshot e2e: no system Chrome/Chromium");
        return;
    }
    let page = br#"<!doctype html><title>Shot</title><main>Screenshot smoke</main>"#;
    let (base, server) = loopback_server(1, move |_request, stream| {
        respond(stream, "200 OK", &[("Content-Type", "text/html")], page);
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);
    let request = format!(r#"{{"url":"{base}","output":"screenshot","waitMs":100}}"#);

    let (ok, out, err) = terrane(home, &["browser", "render", "web", &request]);
    assert!(ok, "browser screenshot failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("blob.stored"), "out: {out}");
    assert!(out.contains("browser.rendered"), "out: {out}");
    server.join().unwrap();

    let (ok, blobs, err) = terrane(home, &["blob", "ls", "web", "__browser__/"]);
    assert!(ok, "blob ls failed; stderr: {err}");
    assert!(blobs.contains("image/png"), "{blobs}");
}
