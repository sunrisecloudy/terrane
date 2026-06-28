//! e2e smoke for `kv`. Logic detail is covered by `rust/crates/terrane-core/tests/cap/kv.rs`.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn kv_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);

    let (ok, out, err) = terrane(home, &["kv", "set", "notes", "theme", "dark"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("kv.set"), "out: {out}");
}
