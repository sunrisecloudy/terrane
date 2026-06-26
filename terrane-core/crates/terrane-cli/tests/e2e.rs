//! End-to-end tests: drive the real `terrane` binary against a throwaway
//! `$TERRANE_HOME`, asserting on stdout/stderr/exit and on the persisted log and
//! state. One per capability.
//!
//! `app` and `kv` are pure, so they run in the default suite. `net` and `model`
//! perform *real* effects (a live HTTP GET, a real agent CLI), so they are
//! `#[ignore]`d — opt in with `cargo test -p terrane-cli -- --ignored`.

use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

/// Run the built `terrane` binary with `args` against `home`; capture the result.
fn terrane(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(args)
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// True if `bin` can be spawned (i.e. is installed and on PATH).
fn on_path(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

#[test]
fn app_capability_e2e() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, _) = terrane(home, &["app", "add", "notes", "Notes Lite", "--source", "apps/notes"]);
    assert!(ok);
    assert!(out.contains("app.added"), "out: {out}");

    let (_, state, _) = terrane(home, &["state"]);
    assert!(state.contains("notes — Notes Lite"), "state: {state}");
    assert!(state.contains("apps/notes"), "state: {state}");

    let (_, log, _) = terrane(home, &["log"]);
    assert!(log.contains("app.added notes"), "log: {log}");

    let (ok, replay, _) = terrane(home, &["replay"]);
    assert!(ok);
    assert!(replay.contains("replay ok"), "replay: {replay}");

    // Duplicate add fails with a non-zero exit and a clear message.
    let (ok, _, err) = terrane(home, &["app", "add", "notes", "Dup"]);
    assert!(!ok);
    assert!(err.contains("already exists"), "err: {err}");

    // Remove, then the catalog is empty; removing a ghost fails.
    let (ok, out, _) = terrane(home, &["app", "remove", "notes"]);
    assert!(ok);
    assert!(out.contains("app.removed"), "out: {out}");

    let (_, state, _) = terrane(home, &["state"]);
    assert!(state.contains("apps:\n  (none)"), "state: {state}");

    let (ok, _, err) = terrane(home, &["app", "remove", "ghost"]);
    assert!(!ok);
    assert!(err.contains("not found"), "err: {err}");
}

#[test]
fn kv_capability_e2e() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);

    let (ok, out, _) = terrane(home, &["kv", "set", "notes", "theme", "dark"]);
    assert!(ok);
    assert!(out.contains("kv.set"), "out: {out}");

    let (_, state, _) = terrane(home, &["state"]);
    assert!(state.contains("notes/theme = dark"), "state: {state}");

    // Setting on an unknown app fails.
    let (ok, _, err) = terrane(home, &["kv", "set", "ghost", "k", "v"]);
    assert!(!ok);
    assert!(err.contains("not found"), "err: {err}");

    // Deleting a missing key fails; deleting a present key works.
    let (ok, _, err) = terrane(home, &["kv", "rm", "notes", "ghost"]);
    assert!(!ok);
    assert!(err.contains("key not found"), "err: {err}");

    let (ok, out, _) = terrane(home, &["kv", "rm", "notes", "theme"]);
    assert!(ok);
    assert!(out.contains("kv.deleted"), "out: {out}");

    // Removing the app cascades to its data.
    terrane(home, &["kv", "set", "notes", "lang", "en"]);
    terrane(home, &["app", "remove", "notes"]);
    let (_, state, _) = terrane(home, &["state"]);
    assert!(state.contains("kv:\n  (none)"), "state: {state}");

    let (ok, _, _) = terrane(home, &["replay"]);
    assert!(ok);
}

#[test]
#[ignore = "real network fetch; run with `cargo test -- --ignored`"]
fn net_capability_e2e_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let (ok, out, err) = terrane(home, &["net", "fetch", "web", "http://example.com"]);
    assert!(ok, "fetch failed; stderr: {err}");
    assert!(out.contains("net.fetched"), "out: {out}");

    let (_, log, _) = terrane(home, &["log"]);
    assert!(
        log.contains("net.fetched web http://example.com"),
        "log: {log}"
    );

    // Replay rebuilds the recorded response from the log — no second fetch.
    let (ok, replay, _) = terrane(home, &["replay"]);
    assert!(ok);
    assert!(replay.contains("replay ok"), "replay: {replay}");
}

#[test]
#[ignore = "real agent call (needs claude on PATH + auth; costs tokens); run with `--ignored`"]
fn model_capability_e2e_real() {
    if !on_path("claude") {
        eprintln!("skipping model e2e: `claude` not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "asst", "Assistant"]);

    let (ok, out, err) = terrane(
        home,
        &["model", "ask", "asst", "claude", "Reply with exactly the two characters: OK"],
    );
    assert!(ok, "agent call failed; stderr: {err}");
    assert!(out.contains("model.responded"), "out: {out}");

    let (_, log, _) = terrane(home, &["log"]);
    assert!(
        log.contains("model.responded asst via claude"),
        "log: {log}"
    );

    // Replay rebuilds the recorded transcript — the agent is not re-run.
    let (ok, replay, _) = terrane(home, &["replay"]);
    assert!(ok);
    assert!(replay.contains("replay ok"), "replay: {replay}");
}
