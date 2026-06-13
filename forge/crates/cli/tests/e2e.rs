//! M0a executable-spine acceptance proof (prd-merged/09 M0a exit).
//!
//! This is THE test that proves the whole jewel runs end to end and offline:
//!
//! ```text
//!   TS ─SWC─▶ JS ─QuickJS─▶ ctx capability gate ─▶ SQLite write
//!       ─▶ UI tree patch ─▶ deterministic RunRecord ─▶ replay (byte-identical)
//! ```
//!
//! It drives the same library entrypoint the `forge demo` binary uses, so a
//! green run here is a green binary. The assertions cover each spine link:
//!   1. the applet installs (TS compiled + manifest validated),
//!   2. the run completes ({ ok: true }),
//!   3. a record is stored (the SQLite write),
//!   4. a UI patch / tree was produced (the UI link),
//!   5. replay is byte-identical (the determinism link).

use forge_cli::{demo, run_demo};

#[test]
fn m0a_spine_runs_end_to_end_and_replays_identically() {
    let outcome = run_demo(serde_json::json!({ "title": "Buy milk" }))
        .expect("the spine must run without a CoreError");

    // 2. run completed ok.
    assert!(outcome.run_ok, "the run's main() must return ok:true");
    assert_eq!(
        outcome.result["value"]["count"],
        serde_json::json!(1),
        "the applet reports the one note it stored"
    );

    // 3. a record was stored (the SQLite write link).
    assert_eq!(outcome.notes.len(), 1, "exactly one note record stored");
    assert_eq!(
        outcome.notes[0]["fields"]["title"],
        serde_json::json!("Buy milk"),
        "the stored note carries the input title"
    );

    // 4. a UI tree/patch was produced (the UI tree-patch link).
    assert!(!outcome.ui_trees.is_empty(), "the run rendered at least one UI tree");
    let tree = outcome.ui_trees[0].to_string();
    assert!(tree.contains("\"type\":\"Stack\""), "root is a Stack: {tree}");
    assert!(tree.contains("\"Notes\""), "header text rendered: {tree}");
    assert!(tree.contains("\"Buy milk\""), "note title in the List: {tree}");

    // 5. replay is byte-identical (the determinism link).
    assert!(
        outcome.replay_identical,
        "the recorded run must replay byte-identically (fingerprint equal)"
    );
    assert!(!outcome.fingerprint.is_empty(), "replay produced a fingerprint");
}

/// Two independent demo runs of the same input produce the same observable
/// fingerprint — the spine is deterministic across fresh workspaces, not just
/// within one record/replay pair.
#[test]
fn the_spine_is_deterministic_across_independent_runs() {
    let a = run_demo(serde_json::json!({ "title": "Same" })).unwrap();
    let b = run_demo(serde_json::json!({ "title": "Same" })).unwrap();
    assert_eq!(
        a.fingerprint, b.fingerprint,
        "same input → same replay fingerprint across independent workspaces"
    );
}

/// `forge demo` (the binary's code path) runs to success and prints the
/// REPLAY IDENTICAL assertion — the harness the M0a exit criterion names.
#[test]
fn forge_demo_runs_to_success_and_prints_the_replay_assertion() {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = demo(&mut buf).expect("forge demo must not error");
    assert!(outcome.run_ok && outcome.replay_identical, "demo asserts the spine");

    let report = String::from_utf8(buf).unwrap();
    assert!(report.contains("REPLAY IDENTICAL: true"), "report:\n{report}");
    assert!(report.contains("emitted UI tree"), "report shows the UI tree");
    assert!(report.contains("stored `notes` records"), "report shows the records");
}
