//! e2e smoke for the external MCP client capability.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread;

use tempfile::tempdir;

use crate::helpers::terrane;

fn loopback_mcp<F>(requests: usize, handler: F) -> (String, thread::JoinHandle<()>)
where
    F: Fn(String) -> String + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let handler = std::sync::Arc::new(handler);
    let handle = thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let request = read_request(&mut stream);
            let body = handler(request);
            respond(stream, &body);
        }
    });
    (addr, handle)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if let Some(done) = request_complete(&bytes) {
            if done {
                break;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn request_complete(bytes: &[u8]) -> Option<bool> {
    let marker = b"\r\n\r\n";
    let header_end = bytes.windows(marker.len()).position(|w| w == marker)?;
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_len = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    Some(bytes.len() >= header_end + marker.len() + content_len)
}

fn respond(mut stream: TcpStream, body: &str) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
}

fn connect_and_grant(home: &Path, name: &str, transport: &str) {
    let (ok, out, err) = terrane(home, &["mcp", "connect", name, transport]);
    assert!(ok, "mcp connect failed; stdout: {out}; stderr: {err}");
    let (ok, out, err) = terrane(home, &["auth", "grant", "user:local-owner", "work", &format!("mcp:{name}")]);
    assert!(ok, "auth grant failed; stdout: {out}; stderr: {err}");
}

#[test]
fn mcp_http_call_records_result_and_replays() {
    let (base, server) = loopback_mcp(1, |request| {
        assert!(request.contains(r#""method":"tools/call""#), "{request}");
        assert!(request.contains(r#""name":"echo""#), "{request}");
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"echo ok"}],"isError":false}}"#.to_string()
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "work", "Work"]);
    let transport = format!(r#"{{"http":{{"url":"{base}"}}}}"#);
    connect_and_grant(home, "echo", &transport);

    let (ok, out, err) = terrane(home, &["mcp", "call", "work", "echo", "echo", r#"{"text":"hello"}"#]);
    assert!(ok, "mcp call failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("mcp.called"), "out: {out}");
    server.join().unwrap();

    let (ok, state, err) = terrane(home, &["state"]);
    assert!(ok, "state failed; stderr: {err}");
    assert!(state.contains("mcp connections:"), "state: {state}");
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(log.contains("mcp.called echo echo"), "log: {log}");
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed; stdout: {out}; stderr: {err}");
}

#[test]
fn mcp_http_tool_error_is_recorded_as_fact() {
    let (base, server) = loopback_mcp(1, |_request| {
        r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"tool failed"}],"isError":true}}"#.to_string()
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "work", "Work"]);
    let transport = format!(r#"{{"http":{{"url":"{base}"}}}}"#);
    connect_and_grant(home, "echo", &transport);

    let (ok, out, err) = terrane(home, &["mcp", "call", "work", "echo", "fail", "{}"]);
    assert!(ok, "mcp call failed; stdout: {out}; stderr: {err}");
    server.join().unwrap();

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(log.contains("error=true"), "log: {log}");
}

#[test]
fn mcp_call_without_exact_grant_is_denied_before_edge() {
    let (base, server) = loopback_mcp(0, |_request| "{}".to_string());
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "work", "Work"]);
    let transport = format!(r#"{{"http":{{"url":"{base}"}}}}"#);
    terrane(home, &["mcp", "connect", "echo", &transport]);

    let (ok, _out, err) = terrane(home, &["mcp", "call", "work", "echo", "echo", "{}"]);
    assert!(!ok, "ungranted mcp call should fail");
    assert!(err.contains("mcp:echo"), "err: {err}");
    server.join().unwrap();
}

#[test]
fn mcp_http_large_result_offloads_to_blob() {
    let large_text = "x".repeat(300 * 1024);
    let (base, server) = loopback_mcp(1, move |_request| {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"{large_text}"}}],"isError":false}}}}"#
        )
    });
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "work", "Work"]);
    let transport = format!(r#"{{"http":{{"url":"{base}"}}}}"#);
    connect_and_grant(home, "echo", &transport);

    let (ok, out, err) = terrane(home, &["mcp", "call", "work", "echo", "large", "{}"]);
    assert!(ok, "mcp call failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("blob.stored"), "out: {out}");
    assert!(out.contains("mcp.called"), "out: {out}");
    server.join().unwrap();
}

#[test]
fn mcp_stdio_call_records_result() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "work", "Work"]);
    let server = home.join("stdio-mcp.sh");
    std::fs::write(
        &server,
        r#"#!/bin/sh
IFS= read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"0.1.0"}}}'
IFS= read -r line
IFS= read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"stdio ok"}],"isError":false}}'
sleep 1
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(&server).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&server, perms).unwrap();
    }
    let transport = format!(r#"{{"stdio":{{"cmd":"{}"}}}}"#, server.display());
    connect_and_grant(home, "stdio", &transport);

    let (ok, out, err) = terrane(home, &["mcp", "call", "work", "stdio", "echo", "{}"]);
    assert!(ok, "mcp stdio call failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("mcp.called"), "out: {out}");
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed; stderr: {err}");
    assert!(log.contains("mcp.called stdio echo"), "log: {log}");
}
