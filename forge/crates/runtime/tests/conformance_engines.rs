//! CR-12 cross-engine conformance harness (T043).
//!
//! This is the **engine-agnostic** harness: it drives each divergence-prone
//! vector in `forge/fixtures/conformance-engines/*.json` through the SAME
//! [`record_run`]/[`replay`] spine — which runs `main(ctx, input)` through the
//! [`JsEngine`](forge_runtime) trait — and asserts the observable output (return
//! value + recorded host-call trace, captured by `RunRecord::replay_fingerprint`)
//! is BYTE-IDENTICAL to the baked-in expectation. Any engine implementation
//! (today QuickJS, tomorrow JavaScriptCore) is held to exactly these vectors, so
//! the same program must produce the same deterministic output on every engine
//! (prd-merged/01 CR-12, `forge/spec/cross-engine-conformance.md`).
//!
//! Each vector additionally proves DETERMINISM end to end:
//!   * run-to-run stability — re-recording the same vector yields the same
//!     fingerprint (no wall-clock, no `Math.random`, deterministic key/iteration
//!     order);
//!   * record→replay identity — replaying the recorded run reproduces it
//!     byte-identically.
//!
//! The corpus `manifest.json` is the source of truth; a guard asserts every
//! declared case is exercised, so a newly added vector fails this suite until it
//! is recorded and its expectation is baked in.

mod common;

use common::owner;
use forge_domain::{Capabilities, DbGrant, Limits, Manifest, RunOutcome, StorageGrant};
use forge_runtime::{record_run, replay, MemoryHostBridge, NullBridge, Program};
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct CorpusManifest {
    cases: Vec<String>,
}

#[derive(serde::Deserialize)]
struct Vector {
    case: String,
    equivalence: String,
    source: Source,
    seeds: Seeds,
    expected: Expected,
}

#[derive(serde::Deserialize)]
struct Source {
    body: String,
    #[serde(rename = "codeHash")]
    code_hash: String,
}

#[derive(serde::Deserialize)]
struct Seeds {
    random_seed: u64,
    time_start: u64,
}

#[derive(serde::Deserialize)]
struct Expected {
    #[serde(rename = "replayFingerprint")]
    replay_fingerprint: String,
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/conformance-engines")
        .canonicalize()
        .expect("conformance-engines fixtures dir exists")
}

/// The conformance manifest (mirrors `fixtures/conformance-engines/*.json`): pure
/// compute + the seeded clock; no storage/db grants so the trace stays engine-
/// agnostic.
fn conformance_manifest() -> Manifest {
    Manifest {
        entrypoint: "inline.js".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities: Capabilities {
            storage: StorageGrant { read: vec![], write: vec![] },
            db: DbGrant { read: vec![], write: vec![] },
            ui: true,
            ..Capabilities::default()
        },
        limits: Limits {
            wall_ms: 30_000, // generous backstop; the corpus runs trivial compute
            fuel: 10_000_000,
            memory_bytes: 67_108_864,
            max_host_calls: 100,
            storage_bytes: 1_048_576,
            log_bytes: 65_536,
        },
        compatibility: Default::default(),
    }
}

fn load_vector(case_file: &str) -> Vector {
    let path = corpus_dir().join(case_file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read vector {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse vector {case_file}: {e}"))
}

/// The whole corpus runs byte-identically through the engine seam, AND every
/// declared manifest case is exercised (the corpus-honesty guard).
#[test]
fn conformance_engines_corpus_is_byte_identical_and_deterministic() {
    let manifest_path = corpus_dir().join("manifest.json");
    let manifest: CorpusManifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read corpus manifest"),
    )
    .expect("parse corpus manifest");
    assert!(
        manifest.cases.len() >= 15,
        "T043 requires ~15 divergence-prone vectors, found {}",
        manifest.cases.len()
    );

    let run_manifest = conformance_manifest();
    let mut ran = 0usize;

    for case_file in &manifest.cases {
        let v = load_vector(case_file);
        let prog = Program::new(forge_domain::AppletId::new("app_conformance"), &v.source.body);

        // The fixture's codeHash must match the program's canonical hash (the
        // body in the fixture IS the executed source — a divergence would mean a
        // stale fixture).
        assert_eq!(
            prog.code_hash(),
            v.source.code_hash,
            "[{}] fixture codeHash is stale vs the source body",
            v.case
        );

        // 1. RECORD through the engine seam. The produced observable fingerprint
        //    must be byte-identical to the baked-in expectation — this is the
        //    cross-engine contract: any engine running this vector must produce
        //    exactly this return value + host-call trace.
        let mut bridge = MemoryHostBridge::new();
        let record = record_run(
            &prog,
            &run_manifest,
            &owner(),
            &serde_json::json!({}),
            v.seeds.random_seed,
            v.seeds.time_start,
            &mut bridge,
        )
        .unwrap_or_else(|e| panic!("[{}] failed to record: {e}", v.case));

        // Every conformance vector is a clean, completing run (the divergence is
        // in the COMPUTED output, normalized at the boundary — never a host crash).
        assert!(
            matches!(record.outcome, RunOutcome::Completed { .. }),
            "[{}] conformance vector must complete, got {:?}",
            v.case,
            record.outcome
        );
        assert_eq!(
            record.replay_fingerprint(),
            v.expected.replay_fingerprint,
            "[{}] observable output diverged from the pinned cross-engine expectation",
            v.case
        );

        // 2. RUN-TO-RUN STABILITY: re-recording yields the same fingerprint (no
        //    wall-clock, no unseeded randomness, deterministic ordering).
        let mut bridge2 = MemoryHostBridge::new();
        let record2 = record_run(
            &prog,
            &run_manifest,
            &owner(),
            &serde_json::json!({}),
            v.seeds.random_seed,
            v.seeds.time_start,
            &mut bridge2,
        )
        .unwrap();
        assert_eq!(
            record.replay_fingerprint(),
            record2.replay_fingerprint(),
            "[{}] re-recording must be byte-identical (determinism)",
            v.case
        );

        // 3. RECORD→REPLAY IDENTITY: replaying reproduces the run byte-identically.
        let mut null = NullBridge::new();
        let replayed = replay(&record, &prog, &run_manifest, &owner(), &mut null)
            .unwrap_or_else(|e| panic!("[{}] failed to replay: {e}", v.case));
        assert!(
            record.replays_identically(&replayed),
            "[{}] record→replay must be byte-identical",
            v.case
        );

        // The equivalence tag is one of the two documented classes.
        assert!(
            v.equivalence == "required_identical" || v.equivalence == "normalized",
            "[{}] equivalence must be required_identical|normalized, got {:?}",
            v.case,
            v.equivalence
        );

        ran += 1;
    }

    assert_eq!(
        ran,
        manifest.cases.len(),
        "every declared conformance case must be exercised"
    );
}
