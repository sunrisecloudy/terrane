//! e2e smoke for the `history` CLI surface.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn history_cli_dry_run_and_apply_revert() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "notes", "Notes"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["kv", "set", "notes", "title", "one"]);
    assert!(ok, "kv set #1 failed: {err}");
    let (ok, out, err) = terrane(home, &["history", "notes", "--key", "title"]);
    assert!(ok, "history key failed: {err}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let to_seq = json["items"][0]["seq"].as_u64().unwrap().to_string();

    let (ok, _, err) = terrane(home, &["kv", "set", "notes", "title", "two"]);
    assert!(ok, "kv set #2 failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &["revert", "notes", "--to", &to_seq, "--key", "title"],
    );
    assert!(ok, "revert dry-run failed: {err}");
    assert!(out.contains("would append 2 event(s)"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "revert", "notes", "--to", &to_seq, "--key", "title", "--yes",
        ],
    );
    assert!(ok, "revert apply failed: {err}");
    assert!(out.contains("kv.set"), "out: {out}");
    assert!(out.contains("history.reverted"), "out: {out}");

    let (ok, out, err) = terrane(home, &["history", "notes", "--key", "title", "--at", &to_seq]);
    assert!(ok, "history at failed: {err}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["value"], "one");
}
