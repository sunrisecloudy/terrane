//! Edge-runner timeout coverage for effectful CLI calls.

use std::io::Read;
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::Duration;

use tempfile::tempdir;

#[test]
fn net_fetch_times_out_when_peer_never_finishes_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 128];
        let _ = stream.read(&mut request);
        thread::sleep(Duration::from_secs(2));
    });

    let home = tempdir().unwrap();
    let add = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(["app", "add", "web", "Web"])
        .env("TERRANE_HOME", home.path())
        .output()
        .expect("spawn terrane app add");
    assert!(
        add.status.success(),
        "app add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let fetch = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(["net", "fetch", "web", &format!("http://{addr}/hang")])
        .env("TERRANE_HOME", home.path())
        .env("TERRANE_EDGE_TIMEOUT_MS", "150")
        .output()
        .expect("spawn terrane net fetch");

    assert!(
        !fetch.status.success(),
        "hung fetch should fail once the read timeout fires"
    );
    assert!(
        !fetch.stderr.is_empty(),
        "hung fetch should surface an error message"
    );

    server.join().unwrap();
}
