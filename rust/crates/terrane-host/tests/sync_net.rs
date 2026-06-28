//! Networked sync e2e: two replicas exchange edits over a real TCP socket (the
//! server runs in a thread on `127.0.0.1:0`) and converge with no lost writes.

use std::net::{TcpListener, TcpStream};
use std::thread;

use tempfile::tempdir;
use terrane_core::cap::crdt::crdt_list_strings;
use terrane_core::Core;
use terrane_domain::Request;
use terrane_host::{serve_conn, sync_conn, EdgeRunner};

fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}

/// A replica with a stable identity, the `notes` app, and one todo pushed.
fn replica_with(path: std::path::PathBuf, item: &str) -> Core<EdgeRunner> {
    let mut core = Core::open_with(path, EdgeRunner).unwrap();
    core.dispatch(req("replica.init", &[])).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("crdt.listPush", &["notes", "todos", item]))
        .unwrap();
    core
}

#[test]
fn two_running_replicas_sync_over_tcp() {
    let dir = tempdir().unwrap();
    let bob_path = dir.path().join("b.bin");
    // Persist Bob's initial state, then drop it — `Core` isn't `Send`, so the
    // server thread reopens it from the log rather than receiving it.
    drop(replica_with(bob_path.clone(), "walk dog"));
    let mut alice = replica_with(dir.path().join("a.bin"), "buy milk");

    // Bob serves on an ephemeral port; Alice connects and syncs.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_path = bob_path.clone();
    let server = thread::spawn(move || {
        let mut bob = Core::open_with(serve_path, EdgeRunner).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        serve_conn(&mut bob, &mut stream).unwrap();
    });

    let mut stream = TcpStream::connect(addr).unwrap();
    let changed = sync_conn(&mut alice, "notes", &mut stream).unwrap();
    drop(stream);
    server.join().unwrap();

    assert!(changed, "alice should have merged bob's edit");

    // Reopen Bob from the log the server thread wrote, and compare.
    let bob = Core::open(&bob_path).unwrap();
    let alice_items = crdt_list_strings(alice.state(), "notes", "todos");
    let bob_items = crdt_list_strings(bob.state(), "notes", "todos");
    for (who, items) in [("alice", &alice_items), ("bob", &bob_items)] {
        assert!(items.contains(&"buy milk".to_string()), "{who}: {items:?}");
        assert!(items.contains(&"walk dog".to_string()), "{who}: {items:?}");
    }
    assert!(alice.state().crdt == bob.state().crdt, "replicas converged");

    // Merged updates still rebuild from each log.
    assert!(alice.replay_matches().unwrap());
    assert!(bob.replay_matches().unwrap());
}
