//! e2e for the `kv` capability — runs in the default suite (pure).

use tempfile::tempdir;

use crate::helpers::terrane;

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
