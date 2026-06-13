//! Data-driven golden-tree conformance test (prd-merged/05 UI-12).
//!
//! Drives the fixture corpus delivered under `tests/golden/` (task T005,
//! authored by Codex) so the Rust diff/patch implementation matches the shared
//! wire contract exactly:
//!   - `roundtrip` cases must serialize→deserialize→serialize identically.
//!   - `diff` cases must produce EXACTLY `expect_patches` (minimal, index-path).
//!   - `unknown` cases must never error and must round-trip the fallback (UI-6).
//!
//! The corpus is the renderer-conformance seed (UI-14) later, so an exact match
//! here is load-bearing.

use forge_ui::{apply, diff, Node, Patch};
use std::fs;
use std::path::{Path, PathBuf};

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Parse a `Node` from a `serde_json::Value`, asserting it never errors (the
/// crate guarantees this for any object, UI-6).
fn node_from(value: &serde_json::Value, ctx: &str) -> Node {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{ctx}: node parse failed (must not): {e}"))
}

#[test]
fn manifest_lists_every_golden_file() {
    let dir = golden_dir();
    let manifest = read_json(&dir.join("manifest.json"));
    let cases = manifest["cases"].as_array().expect("manifest.cases array");

    // Every referenced file exists.
    for case in cases {
        let file = case["file"].as_str().expect("case.file string");
        assert!(
            dir.join(file).is_file(),
            "manifest references missing fixture {file}"
        );
    }

    // Every *.json fixture (except the manifest) is referenced.
    let listed: std::collections::BTreeSet<String> = cases
        .iter()
        .map(|c| c["file"].as_str().unwrap().to_string())
        .collect();
    for entry in fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        if name == "manifest.json" || !name.ends_with(".json") {
            continue;
        }
        assert!(listed.contains(&name), "fixture {name} not in manifest");
    }

    assert!(cases.len() >= 16, "expected the ~16-22 case corpus, got {}", cases.len());
}

#[test]
fn all_golden_cases_pass() {
    let dir = golden_dir();
    let manifest = read_json(&dir.join("manifest.json"));
    let cases = manifest["cases"].as_array().unwrap();

    let mut roundtrip = 0usize;
    let mut diffs = 0usize;
    let mut unknowns = 0usize;

    for case in cases {
        let file = case["file"].as_str().unwrap();
        let fixture = read_json(&dir.join(file));
        let kind = fixture["kind"].as_str().unwrap_or("");
        let ctx = file.to_string();

        match kind {
            "roundtrip" => {
                run_roundtrip(&fixture, &ctx);
                roundtrip += 1;
            }
            "diff" => {
                run_diff(&fixture, &ctx);
                diffs += 1;
            }
            "unknown" => {
                run_unknown(&fixture, &ctx);
                unknowns += 1;
            }
            other => panic!("{ctx}: unknown fixture kind {other:?}"),
        }
    }

    // Sanity: the corpus exercises all three case kinds.
    assert!(roundtrip > 0 && diffs > 0 && unknowns > 0,
        "expected all three kinds; got roundtrip={roundtrip} diff={diffs} unknown={unknowns}");
}

/// `roundtrip`: tree must serialize→deserialize→serialize identically.
fn run_roundtrip(fixture: &serde_json::Value, ctx: &str) {
    let tree_json = &fixture["tree"];
    let node = node_from(tree_json, ctx);
    let reser: serde_json::Value =
        serde_json::to_value(&node).unwrap_or_else(|e| panic!("{ctx}: reserialize: {e}"));
    assert_eq!(
        &reser, tree_json,
        "{ctx}: tree did not round-trip identically"
    );
    // And the typed value is stable across a second parse.
    let node2 = node_from(&reser, ctx);
    assert_eq!(node, node2, "{ctx}: typed value unstable across roundtrip");
}

/// `diff`: diff(old,new) must equal `expect_patches` exactly, and applying them
/// to `old` must reproduce `new`.
fn run_diff(fixture: &serde_json::Value, ctx: &str) {
    let old = node_from(&fixture["old"], ctx);
    let new = node_from(&fixture["new"], ctx);

    let expected: Vec<Patch> = fixture["expect_patches"]
        .as_array()
        .unwrap_or_else(|| panic!("{ctx}: expect_patches array"))
        .iter()
        .map(|p| {
            serde_json::from_value(p.clone())
                .unwrap_or_else(|e| panic!("{ctx}: parse expected patch {p}: {e}"))
        })
        .collect();

    let actual = diff(Some(&old), &new);
    assert_eq!(
        actual, expected,
        "{ctx}: diff mismatch\n  expected: {expected:?}\n  actual:   {actual:?}"
    );

    // Wire shapes must serialize back to the fixture's exact patch JSON.
    let actual_json: serde_json::Value = serde_json::to_value(&actual).unwrap();
    assert_eq!(
        &actual_json, &fixture["expect_patches"],
        "{ctx}: serialized patch wire shape mismatch"
    );

    // apply(diff(old,new)) == new (round-trip property, UI-1).
    let mut applied = old.clone();
    apply(&mut applied, &actual).unwrap_or_else(|e| panic!("{ctx}: apply failed: {e}"));
    assert_eq!(applied, new, "{ctx}: apply(diff) did not reproduce new");
}

/// `unknown`: tree must parse without error and survive a self-diff, honoring
/// `must_not_error` (UI-6).
fn run_unknown(fixture: &serde_json::Value, ctx: &str) {
    assert!(
        fixture["must_not_error"].as_bool().unwrap_or(false),
        "{ctx}: unknown fixture must assert must_not_error"
    );
    let node = node_from(&fixture["tree"], ctx);

    // Self-diff must be empty and never panic.
    assert!(
        diff(Some(&node), &node).is_empty(),
        "{ctx}: self-diff of unknown-bearing tree should be empty"
    );

    // Re-serialize and re-parse; known/unknown subtrees survive.
    let reser: serde_json::Value = serde_json::to_value(&node).unwrap();
    let node2 = node_from(&reser, ctx);
    assert_eq!(node, node2, "{ctx}: unknown-bearing tree unstable across roundtrip");
}
