//! Smoke test for the `terrane-host` binary: its top-level `run` verb executes
//! the real `apps/todo-cli` JS backend (the UI-free CLI app) and the world
//! replays. The exhaustive logic lives in terrane-core/tests/cap/host.rs; this
//! just proves the host front door.

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

fn todo_cli_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")) // host/cli
        .join("../../apps/todo-cli") // repo-root/apps/todo-cli
        .canonicalize()
        .expect("apps/todo-cli bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn terrane_host_runs_todo_cli_backend() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = todo_cli_source();

    assert!(
        host(
            home,
            &["app", "add", "todo-cli", "Todo (CLI)", "--source", &src]
        )
        .0
    );

    let (ok, out, err) = host(home, &["run", "todo-cli", "add", "buy milk"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "added #1 buy milk", "out: {out}");

    let (ok, out, _) = host(home, &["run", "todo-cli", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#1 buy milk", "out: {out}");

    assert!(host(home, &["replay"]).0, "replay failed");
}
