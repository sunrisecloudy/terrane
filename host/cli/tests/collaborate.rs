//! E2E for the `terrane-host` binary running the real `apps/todo-cli-collaborate`
//! backend — the CRDT-backed CLI todo. Proves the app's verbs work through the
//! real front door AND that the world replays: the only records a run produces
//! are `crdt.update` events, so `replay` rebuilding state without re-running JS
//! is the end-to-end proof that the crdt storage path is sound (Option A).
//!
//! The CRDT *merge* guarantee (two replicas converge with no lost writes) is
//! proven against the same `crdt.listPush` capability this app uses in
//! terrane-core/tests/cap/crdt.rs (`two_app_replicas_merge_with_no_lost_writes`).

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

fn collaborate_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")) // host/cli
        .join("../../apps/todo-cli-collaborate") // repo-root/apps/todo-cli-collaborate
        .canonicalize()
        .expect("apps/todo-cli-collaborate bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn terrane_host_runs_collaborative_todo_backend() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = collaborate_source();

    assert!(
        host(
            home,
            &["app", "add", "todo-cli-collaborate", "Todo", "--source", &src]
        )
        .0
    );

    // add → list across runs (state persists via recorded crdt.update events).
    let (ok, out, err) = host(home, &["run", "todo-cli-collaborate", "add", "buy milk"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "added: buy milk");

    assert_eq!(
        host(home, &["run", "todo-cli-collaborate", "add", "walk dog"]).1.trim(),
        "added: walk dog"
    );

    let (_, out, _) = host(home, &["run", "todo-cli-collaborate", "list"]);
    assert_eq!(out.trim(), "#1 buy milk\n#2 walk dog", "out: {out}");

    // done removes by 1-based number; the list renumbers.
    let (_, out, _) = host(home, &["run", "todo-cli-collaborate", "done", "1"]);
    assert_eq!(out.trim(), "done #1 buy milk", "out: {out}");
    let (_, out, _) = host(home, &["run", "todo-cli-collaborate", "list"]);
    assert_eq!(out.trim(), "#1 walk dog", "out: {out}");

    // done with a bad / out-of-range number is a clean message, not a crash.
    let (ok, out, _) = host(home, &["run", "todo-cli-collaborate", "done", "9"]);
    assert!(ok);
    assert_eq!(out.trim(), "no todo #9", "out: {out}");

    // Option A: the crdt-backed app rebuilds purely from its recorded updates.
    assert!(host(home, &["replay"]).0, "replay failed");
}
