//! Corpus-driven conformance: every adversarial sample marked
//! `expected_outcome == "rejected_static"` in the runtime crate's corpus must be
//! rejected by [`forge_pipeline::enforce_policy`] *before* execution
//! (prd-merged/01 CR-13 layer 1, prd-merged/04 LM-9), and the surviving
//! (non-static) samples must transpile cleanly.
//!
//! The corpus is owned by `crates/runtime`; this test only *reads* it so the two
//! crates agree on the forbidden set without duplicating fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use forge_pipeline::{compile, enforce_policy, policy_scan};

fn corpus_dir() -> PathBuf {
    // crates/pipeline -> crates/runtime/tests/corpus
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("runtime")
        .join("tests")
        .join("corpus")
}

#[derive(serde::Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    file: String,
    expected_outcome: String,
}

fn load_manifest() -> Manifest {
    let path = corpus_dir().join("manifest.json");
    let raw = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read corpus manifest {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("parse corpus manifest")
}

fn read_case(file: &str) -> String {
    let path = corpus_dir().join(file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read corpus case {file}: {e}"))
}

#[test]
fn every_rejected_static_case_is_rejected_and_named() {
    let manifest = load_manifest();
    let mut checked = 0usize;

    for case in &manifest.cases {
        if case.expected_outcome != "rejected_static" {
            continue;
        }
        checked += 1;
        let src = read_case(&case.file);

        // The strict gate must error.
        let err = enforce_policy(&src)
            .expect_err(&format!("{} should be rejected by static policy", case.file));
        // Network/host-escape + tamper all map to a capability/safety boundary.
        assert_eq!(
            err.code(),
            "PermissionDenied",
            "{} rejected with the wrong error kind: {err:?}",
            case.file
        );

        // The report must name at least one construct + reason.
        let findings = policy_scan(&src).expect("scan should parse");
        assert!(
            !findings.is_empty(),
            "{} produced no findings",
            case.file
        );
        for f in &findings {
            assert!(!f.construct.is_empty(), "{}: empty construct", case.file);
            assert!(!f.reason.is_empty(), "{}: empty reason", case.file);
        }
    }

    // The manifest currently lists 10 rejected_static cases; guard against the
    // corpus silently shrinking out from under this test.
    assert_eq!(checked, 10, "expected 10 rejected_static corpus cases, saw {checked}");
}

#[test]
fn non_static_corpus_cases_pass_policy_and_transpile() {
    // Cases that are *not* rejected statically (cpu/memory/recursion/flood) are
    // legitimate TypeScript: the policy scan must let them through and the
    // transpiler must accept them. (They are stopped later by runtime limits.)
    let manifest = load_manifest();
    let mut checked = 0usize;

    for case in &manifest.cases {
        if case.expected_outcome == "rejected_static" {
            continue;
        }
        let src = read_case(&case.file);

        // No false positives from the static scan.
        let findings = policy_scan(&src).expect("scan should parse");
        assert!(
            findings.is_empty(),
            "{} should NOT be flagged statically, got {findings:?}",
            case.file
        );

        // And it transpiles cleanly through the full front-of-spine.
        compile(&src).unwrap_or_else(|e| panic!("{} should compile, got {e:?}", case.file));
        checked += 1;
    }

    assert!(checked >= 8, "expected the non-static corpus cases, saw {checked}");
}
