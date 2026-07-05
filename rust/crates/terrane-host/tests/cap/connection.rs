use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;

use tempfile::tempdir;

use crate::helpers::terrane;

fn terrane_file_store(home: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(args)
        .env("TERRANE_HOME", home)
        .env("TERRANE_SECRET_STORE", "file")
        .output()
        .expect("spawn terrane");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn terrane_file_store_stdin(
    home: &std::path::Path,
    args: &[&str],
    stdin: &str,
) -> (bool, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(args)
        .env("TERRANE_HOME", home)
        .env("TERRANE_SECRET_STORE", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrane");
    child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait terrane");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

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

fn respond(mut stream: TcpStream, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

#[test]
fn connection_file_fallback_resolves_secret_for_net_without_log_leak() {
    let (base, server) = loopback_server(1, |request, stream| {
        assert!(request.contains("authorization: Bearer raw-secret"), "{request}");
        respond(stream, b"ok");
    });
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane_file_store_stdin(
        home,
        &["connection", "set", "github", "--field", "key"],
        "Bearer raw-secret\n",
    );
    assert!(ok, "connection set failed; stdout: {out}; stderr: {err}");
    let (ok, _out, err) = terrane_file_store(home, &["app", "add", "web", "Web App"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _out, err) = terrane_file_store(
        home,
        &["auth", "grant", "user:local-owner", "web", "connection:github"],
    );
    assert!(ok, "connection grant failed: {err}");

    let request = format!(
        r#"{{"url":"{base}/items","headers":{{"authorization":{{"$secret":"github"}}}}}}"#
    );
    let (ok, out, err) = terrane_file_store(home, &["net", "request", "web", &request]);
    assert!(ok, "net request failed; stdout: {out}; stderr: {err}");
    server.join().unwrap();

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("connection.defined github"));
    assert!(log.contains("net.responded web GET"));
    assert!(!log.contains("raw-secret"), "log leaked secret: {log}");
    let fallback = std::fs::read_to_string(home.join("secrets.enc")).unwrap();
    assert!(!fallback.contains("raw-secret"));
}

#[test]
fn missing_connection_grant_blocks_resolution() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, out, err) = terrane_file_store_stdin(
        home,
        &["connection", "set", "github", "--field", "key"],
        "Bearer raw-secret\n",
    );
    assert!(ok, "connection set failed; stdout: {out}; stderr: {err}");
    let (ok, _out, err) = terrane_file_store(home, &["app", "add", "web", "Web App"]);
    assert!(ok, "app add failed: {err}");

    let request =
        r#"{"url":"http://127.0.0.1:9/items","headers":{"authorization":{"$secret":"github"}}}"#;
    let (ok, _out, err) = terrane_file_store(home, &["net", "request", "web", request]);
    assert!(!ok, "request should require connection grant");
    assert!(err.contains("permission required: grant connection:github"), "{err}");
}
