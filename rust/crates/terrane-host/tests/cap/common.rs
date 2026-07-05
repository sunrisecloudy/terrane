use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

fn smtp_server() -> (String, u16, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    return capture_smtp(stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return "SMTP SERVER TIMEOUT WAITING FOR CLIENT".to_string();
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return format!("SMTP SERVER ACCEPT ERROR: {e}"),
            }
        }
    });
    ("127.0.0.1".to_string(), addr.port(), handle)
}

fn capture_smtp(mut stream: TcpStream) -> String {
    write!(stream, "220 localhost ESMTP\r\n").unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut data = String::new();
    let mut in_data = false;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        if n == 0 {
            break;
        }
        if in_data {
            if line == ".\r\n" {
                write!(stream, "250 queued\r\n").unwrap();
                in_data = false;
            } else {
                data.push_str(&line);
            }
            continue;
        }
        if line.starts_with("EHLO") {
            write!(stream, "250-localhost\r\n250 AUTH PLAIN\r\n").unwrap();
        } else if line.starts_with("AUTH PLAIN") {
            write!(stream, "235 authenticated\r\n").unwrap();
        } else if line.starts_with("MAIL FROM") || line.starts_with("RCPT TO") {
            write!(stream, "250 ok\r\n").unwrap();
        } else if line.starts_with("DATA") {
            write!(stream, "354 go ahead\r\n").unwrap();
            in_data = true;
        } else if line.starts_with("QUIT") {
            write!(stream, "221 bye\r\n").unwrap();
            break;
        } else {
            write!(stream, "250 ok\r\n").unwrap();
        }
    }
    data
}

#[test]
fn common_send_email_uses_connection_and_blob_attachment_without_secret_log_leak() {
    let (host, port, server) = smtp_server();
    let dir = tempdir().unwrap();
    let home = dir.path();
    let attachment = home.join("report.txt");
    fs::write(&attachment, b"attachment bytes").unwrap();

    let (ok, _, err) = terrane_file_store(home, &["app", "add", "mailbot", "Mail Bot"]);
    assert!(ok, "app add failed: {err}");
    let (ok, out, err) = terrane_file_store(
        home,
        &[
            "blob",
            "put",
            "mailbot",
            "reports/w27.txt",
            "text/plain",
            attachment.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {out} {err}");

    let config = format!(
        r#"{{"host":"{host}","port":{port},"username":"sender@example.com","from":"sender@example.com"}}"#
    );
    let (ok, out, err) = terrane_file_store_stdin(
        home,
        &[
            "connection",
            "set",
            "smtp-default",
            "--kind",
            "smtp",
            "--field",
            "password",
            "--config",
            &config,
        ],
        "smtp-secret\n",
    );
    assert!(ok, "connection set failed: {out} {err}");
    for resource in ["common:send:email", "connection:smtp-default"] {
        let (ok, _, err) = terrane_file_store(
            home,
            &["auth", "grant", "user:local-owner", "mailbot", resource],
        );
        assert!(ok, "grant {resource} failed: {err}");
    }

    let msg = r#"{"channel":"email","to":["a@example.com"],"subject":"Weekly","text":"Hello","attachments":["reports/w27.txt"],"connection":"smtp-default","sentAt":42}"#;
    let (ok, out, err) = terrane_file_store(home, &["common", "send", "mailbot", msg]);
    assert!(ok, "common send failed: {out} {err}");
    assert!(out.contains("common.sent"), "out: {out}");
    let (ok, log_after_send, err) = terrane(home, &["log"]);
    assert!(ok, "log failed after send: {err}");
    assert!(
        log_after_send.contains("status=sent"),
        "send should succeed, log: {log_after_send}"
    );
    let captured = server.join().unwrap();
    assert!(captured.contains("Subject: Weekly"), "{captured}");
    assert!(captured.contains("Content-Disposition: attachment"), "{captured}");

    assert!(log_after_send.contains("common.sent mailbot email"));
    assert!(
        !log_after_send.contains("smtp-secret"),
        "log leaked secret: {log_after_send}"
    );
}
