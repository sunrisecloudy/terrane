//! Multi-user e2e: two independent `TERRANE_HOME`s (two "users") each run the
//! real `apps/todo-cli-collaborate` backend offline, then `terrane-host sync`
//! merges one into the other through the binary. Both converge with no lost
//! writes — the observable proof that the system is genuinely multi-replica.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

fn host(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane-host"))
        .args(args)
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane-host");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn app_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/todo-cli-collaborate")
        .canonicalize()
        .expect("apps/todo-cli-collaborate bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

const APP: &str = "todo-cli-collaborate";

fn install(home: &Path, src: &str) {
    assert!(host(home, &["app", "add", APP, "Todo", "--source", src]).0);
}

#[test]
fn two_homes_converge_after_sync() {
    let src = app_source();
    let alice_dir = tempdir().unwrap();
    let bob_dir = tempdir().unwrap();
    let alice = alice_dir.path();
    let bob = bob_dir.path();

    install(alice, &src);
    install(bob, &src);

    // Concurrent, offline edits in two separate homes.
    assert_eq!(
        host(alice, &["run", APP, "add", "buy milk"]).1.trim(),
        "added: buy milk"
    );
    assert_eq!(
        host(bob, &["run", APP, "add", "walk dog"]).1.trim(),
        "added: walk dog"
    );

    // Before sync each home only sees its own todo.
    assert_eq!(host(alice, &["run", APP, "list"]).1.trim(), "#1 buy milk");
    assert_eq!(host(bob, &["run", APP, "list"]).1.trim(), "#1 walk dog");

    // Alice pulls Bob's edits. Distinct replica peers → no lost write.
    let (ok, out, err) = host(alice, &["sync", APP, "--from", bob.to_str().unwrap()]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("synced"), "out: {out}");

    let alice_list = host(alice, &["run", APP, "list"]).1;
    assert!(alice_list.contains("buy milk"), "alice: {alice_list}");
    assert!(alice_list.contains("walk dog"), "alice: {alice_list}");

    // Re-syncing the same thing changes nothing (idempotent).
    let (_, out, _) = host(alice, &["sync", APP, "--from", bob.to_str().unwrap()]);
    assert!(out.contains("already up to date"), "out: {out}");

    // Sync the other direction; Bob converges to the same set.
    assert!(host(bob, &["sync", APP, "--from", alice.to_str().unwrap()]).0);
    let bob_list = host(bob, &["run", APP, "list"]).1;
    assert!(bob_list.contains("buy milk"), "bob: {bob_list}");
    assert!(bob_list.contains("walk dog"), "bob: {bob_list}");

    // Both homes still replay cleanly (merged updates rebuild from the log).
    assert!(host(alice, &["replay"]).0);
    assert!(host(bob, &["replay"]).0);
}
