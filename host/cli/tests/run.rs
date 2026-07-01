//! Smoke test for the `terrane-host` binary: its top-level `run` verb executes
//! the real `apps/todo-cli` JS backend (the UI-free CLI app) and the world
//! replays. The exhaustive logic lives in rust/crates/terrane-core/tests/cap/host.rs; this
//! just proves the host front door.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

fn host(home: &Path, args: &[&str]) -> (bool, String, String) {
    host_with_env(home, args, &[])
}

fn host_with_env(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane-host"))
        .args(args)
        .env("TERRANE_HOME", home)
        .envs(envs.iter().copied())
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
    let (ok, _, err) = host(
        home,
        &["auth", "grant", "user:local-owner", "todo-cli", "kv"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = host(home, &["run", "todo-cli", "add", "buy milk"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "added #1 buy milk", "out: {out}");

    let (ok, out, _) = host(home, &["run", "todo-cli", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#1 buy milk", "out: {out}");

    let (ok, out, err) = host_with_env(
        home,
        &["run", "todo-cli", "list"],
        &[("TERRANE_PERMISSION_UI", "garbage")],
    );
    assert!(
        ok,
        "invalid permission UI env should not block granted apps: {err}"
    );
    assert_eq!(out.trim(), "#1 buy milk", "out: {out}");

    assert!(host(home, &["replay"]).0, "replay failed");
}

#[test]
fn terrane_host_run_reports_permission_request_and_wait_timeout() {
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

    let (ok, _out, err) = host(
        home,
        &["run", "--permission-ui", "print", "todo-cli", "list"],
    );
    assert!(!ok, "run without grant should fail closed");
    assert!(
        err.contains("permission required")
            && err.contains("request id:")
            && err.contains("/__terrane/admin/requests/")
            && err.contains("source: cli")
            && err.contains("permission_required"),
        "permission stderr should be actionable: {err}"
    );

    let (ok, _out, err) = host(
        home,
        &[
            "run",
            "--permission-ui",
            "none",
            "--permission-wait",
            "--permission-timeout",
            "0",
            "todo-cli",
            "list",
        ],
    );
    assert!(!ok, "wait should fail after timeout");
    assert!(
        err.contains("timed out waiting for request"),
        "timeout stderr: {err}"
    );

    let (ok, _out, err) = host(
        home,
        &[
            "run",
            "--permission-ui",
            "none",
            "--no-open",
            "todo-cli",
            "list",
        ],
    );
    assert!(!ok, "run without grant should fail closed");
    assert!(
        err.contains("permission UI disabled") && !err.contains("/__terrane/admin/requests/"),
        "--no-open should not make permission-ui none verbose: {err}"
    );
}
