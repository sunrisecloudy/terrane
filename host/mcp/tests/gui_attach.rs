use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};

use serde_json::json;
use tempfile::tempdir;

const TOKEN: &str = "gui-attach-test-token";

fn send(stdin: &mut impl Write, json: &str) {
    stdin.write_all(json.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

fn read_request(stream: &mut TcpStream) -> (String, String) {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "HTTP request ended before its body arrived");
        data.extend_from_slice(&chunk[..read]);
        let Some(split) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header_end = split + 4;
        let headers = String::from_utf8(data[..split].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if data.len() >= header_end + content_length {
            let request_line = headers.lines().next().unwrap().to_string();
            assert!(
                headers.contains(&format!("Authorization: Bearer {TOKEN}")),
                "missing GUI bearer token: {headers}"
            );
            let body =
                String::from_utf8(data[header_end..header_end + content_length].to_vec()).unwrap();
            return (request_line, body);
        }
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 202 { "Accepted" } else { "OK" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

fn serve_health(listener: &TcpListener) {
    let (mut stream, _) = listener.accept().unwrap();
    let (request, body) = read_request(&mut stream);
    assert!(request.starts_with("GET /health "), "{request}");
    assert!(body.is_empty());
    respond(&mut stream, 200, r#"{"status":"ready"}"#);
}

fn serve_mcp(listener: &TcpListener, core: &mut terrane_host::HostCore) {
    let (mut stream, _) = listener.accept().unwrap();
    let (request, body) = read_request(&mut stream);
    assert!(request.starts_with("POST /mcp "), "{request}");
    match terrane_host::mcp::handle_json_rpc_with_source(core, &body, "mcp_gui") {
        Some(response) => respond(&mut stream, 200, &response),
        None => respond(&mut stream, 202, ""),
    }
}

#[test]
fn stdio_server_attaches_to_gui_owner_instead_of_failing_the_home_lock() {
    let dir = tempdir().unwrap();
    let home = dir.path().join(".terrane");
    fs::create_dir(&home).unwrap();
    let mut gui_core = terrane_host::open_at_home(&home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::write(
        home.join("mcp-gui.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "endpoint": format!("http://127.0.0.1:{port}/mcp"),
            "health": format!("http://127.0.0.1:{port}/health"),
            "token": TOKEN,
            "pid": std::process::id(),
        }))
        .unwrap(),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_terrane-mcp"))
        .env_remove("TERRANE_HOME")
        .env("HOME", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();

    serve_health(&listener);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gui-attach-test","version":"1"}}}"#,
    );
    serve_mcp(&listener, &mut gui_core);
    let mut initialize = String::new();
    stdout.read_line(&mut initialize).unwrap();
    assert!(
        initialize.contains(r#""name":"terrane-mcp""#),
        "{initialize}"
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    serve_mcp(&listener, &mut gui_core);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#,
    );
    serve_mcp(&listener, &mut gui_core);
    let mut apps = String::new();
    stdout.read_line(&mut apps).unwrap();
    assert!(apps.contains(r#""apps":[]"#), "{apps}");

    drop(stdin);
    assert!(child.wait().unwrap().success());
    let mut diagnostics = String::new();
    stderr.read_to_string(&mut diagnostics).unwrap();
    assert!(
        diagnostics.contains("attached to Terrane GUI"),
        "{diagnostics}"
    );
}
