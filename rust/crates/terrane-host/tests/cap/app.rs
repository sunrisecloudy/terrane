//! e2e smoke for `app` — the real binary parses, dispatches, and reports.
//! Logic detail (replay, cascade, validation) is covered by
//! `rust/crates/terrane-core/tests/cap/app.rs`.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn app_e2e_smoke() {
    let dir = tempdir().unwrap();
    let (ok, out, err) = terrane(dir.path(), &["app", "add", "notes", "Notes Lite"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("app.added"), "out: {out}");
}
