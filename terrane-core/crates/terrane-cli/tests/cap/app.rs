//! e2e for the `app` capability — runs in the default suite (pure).

use tempfile::tempdir;

use crate::helpers::terrane;

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
