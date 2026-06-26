//! CR-12 cross-engine conformance harness (T043).
//!
//! This is the **engine-agnostic** harness: it drives each divergence-prone
//! vector in `forge/fixtures/conformance-engines/*.json` through an injected
//! [`JsEngine`](forge_runtime) implementation — via the engine-parameterized
//! [`record_run_with_engine`]/[`replay_with_engine`] spine, which runs
//! `main(ctx, input)` through the trait — and asserts the observable output
//! (return value + recorded host-call trace, captured by
//! `RunRecord::replay_fingerprint`) is BYTE-IDENTICAL to the baked-in
//! expectation. The corpus-running body [`run_corpus_through_engine`] takes the
//! engine as a `&dyn JsEngine` PARAMETER, so any engine implementation (today
//! `QuickJsEngine`, tomorrow a real JavaScriptCore / QuickJS-WASM backend) is
//! held to exactly these vectors and the same program must produce the same
//! deterministic output on every engine (prd-merged/01 CR-12,
//! `forge/spec/cross-engine-conformance.md`).
//!
//! Wiring a second engine is therefore purely additive: implement the
//! [`JsEngine`] trait and call [`run_corpus_through_engine`] with it. The
//! `second_engine_runs_the_same_corpus_byte_identically` test below proves the
//! seam by running the **entire corpus** through a *distinct* `JsEngine` impl
//! (`AdapterEngine`, a separate type that re-marshals through the trait) and
//! asserting it produces the SAME pinned fingerprints — i.e. the harness is
//! genuinely parameterized over the engine, not hard-wired to `QuickJsEngine`.
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
use forge_domain::{AppResult, Capabilities, DbGrant, Limits, Manifest, RunOutcome, StorageGrant};
use forge_runtime::{
    record_run_with_engine, replay_with_engine, EngineOutcome, HostContext, JsEngine,
    MemoryHostBridge, NullBridge, Program, QuickJsEngine,
};
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
    manifest: Manifest,
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

/// Run the WHOLE corpus through the injected [`JsEngine`] (`engine`), asserting
/// every vector is byte-identical to its pinned cross-engine expectation, that
/// re-recording is stable, and that record→replay is identical — AND that every
/// declared manifest case is exercised (the corpus-honesty guard).
///
/// This is the **engine-agnostic core** of the harness (CR-12): the engine is a
/// `&dyn JsEngine` parameter, threaded into the record/replay spine via
/// [`record_run_with_engine`] / [`replay_with_engine`], so holding a second
/// engine (a real JavaScriptCore / QuickJS-WASM backend) to the corpus is just a
/// matter of passing it here. `engine_label` only enriches failure messages so a
/// divergence names the offending engine.
fn run_corpus_through_engine(engine: &dyn JsEngine, engine_label: &str) {
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

    let mut ran = 0usize;
    // The only two vectors whose observable is a host-normalized projection of an
    // engine-defined shape (error `.message`/`.stack`, stack-limit depth). Every
    // other vector — including `math_determinism_no_random`, whose `Math.random`
    // throw is HOST-installed and identical by construction — is
    // `required_identical`. This guard pins the equivalence-class partition so the
    // fixtures and `spec/cross-engine-conformance.md` (§3 table + rules) cannot
    // drift apart silently.
    const NORMALIZED_CASES: [&str; 2] = ["error_message_normalized", "recursion_stack_limit"];
    let mut normalized_seen: Vec<String> = Vec::new();

    for case_file in &manifest.cases {
        let v = load_vector(case_file);
        v.manifest
            .validate()
            .unwrap_or_else(|e| panic!("[{engine_label}/{}] invalid manifest: {e}", v.case));
        let run_manifest = &v.manifest;
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

        // 1. RECORD through the injected engine. The produced observable
        //    fingerprint must be byte-identical to the baked-in expectation — this
        //    is the cross-engine contract: ANY engine running this vector must
        //    produce exactly this return value + host-call trace.
        let mut bridge = MemoryHostBridge::new();
        let record = record_run_with_engine(
            engine,
            &prog,
            run_manifest,
            &owner(),
            &serde_json::json!({}),
            v.seeds.random_seed,
            v.seeds.time_start,
            &mut bridge,
        )
        .unwrap_or_else(|e| panic!("[{engine_label}/{}] failed to record: {e}", v.case));

        // Every conformance vector is a clean, completing run (the divergence is
        // in the COMPUTED output, normalized at the boundary — never a host crash).
        assert!(
            matches!(record.outcome, RunOutcome::Completed { .. }),
            "[{engine_label}/{}] conformance vector must complete, got {:?}",
            v.case,
            record.outcome
        );
        assert_eq!(
            record.replay_fingerprint(),
            v.expected.replay_fingerprint,
            "[{engine_label}/{}] observable output diverged from the pinned cross-engine expectation",
            v.case
        );

        // 2. RUN-TO-RUN STABILITY: re-recording yields the same fingerprint (no
        //    wall-clock, no unseeded randomness, deterministic ordering).
        let mut bridge2 = MemoryHostBridge::new();
        let record2 = record_run_with_engine(
            engine,
            &prog,
            run_manifest,
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
            "[{engine_label}/{}] re-recording must be byte-identical (determinism)",
            v.case
        );

        // 3. RECORD→REPLAY IDENTITY: replaying reproduces the run byte-identically.
        let mut null = NullBridge::new();
        let replayed = replay_with_engine(engine, &record, &prog, run_manifest, &owner(), &mut null)
            .unwrap_or_else(|e| panic!("[{engine_label}/{}] failed to replay: {e}", v.case));
        assert!(
            record.replays_identically(&replayed),
            "[{engine_label}/{}] record→replay must be byte-identical",
            v.case
        );

        // The equivalence tag is one of the two documented classes.
        assert!(
            v.equivalence == "required_identical" || v.equivalence == "normalized",
            "[{}] equivalence must be required_identical|normalized, got {:?}",
            v.case,
            v.equivalence
        );
        // Cross-check the tag against the spec's fixed normalized partition: a
        // `normalized` tag is only legal on the two cases §3 documents, and those
        // two cases must be tagged `normalized`. This is what stops the
        // fixture/spec/report contradiction (FIX ROUND 2) from recurring silently.
        let is_documented_normalized = NORMALIZED_CASES.contains(&v.case.as_str());
        assert_eq!(
            v.equivalence == "normalized",
            is_documented_normalized,
            "[{}] equivalence tag disagrees with spec §3: this case is {} the \
             documented normalized set {:?} but is tagged {:?}",
            v.case,
            if is_documented_normalized { "in" } else { "NOT in" },
            NORMALIZED_CASES,
            v.equivalence
        );
        if v.equivalence == "normalized" {
            normalized_seen.push(v.case.clone());
        }

        ran += 1;
    }

    assert_eq!(
        ran,
        manifest.cases.len(),
        "every declared conformance case must be exercised"
    );

    // Both documented `normalized` cases must actually be present in the corpus —
    // the partition is exactly these two, no more, no fewer.
    normalized_seen.sort();
    let mut expected_normalized: Vec<String> =
        NORMALIZED_CASES.iter().map(|s| s.to_string()).collect();
    expected_normalized.sort();
    assert_eq!(
        normalized_seen, expected_normalized,
        "the `normalized` equivalence class must be exactly {NORMALIZED_CASES:?} \
         (spec §3); `math_determinism_no_random` is `required_identical`"
    );
}

/// The corpus runs byte-identically through the built-in [`QuickJsEngine`] — the
/// only engine that ships today (prd-merged/01 CR-12). This is the conformance
/// baseline every other engine is compared against.
#[test]
fn conformance_engines_corpus_is_byte_identical_and_deterministic() {
    run_corpus_through_engine(&QuickJsEngine::new(), "QuickJsEngine");
}

/// A SECOND, distinct [`JsEngine`] implementation runs the SAME corpus and
/// produces the SAME pinned fingerprints — proving the harness is genuinely
/// **engine-agnostic** (the engine is a parameter, not hard-wired to
/// `QuickJsEngine`). When a real JavaScriptCore / QuickJS-WASM backend lands it
/// slots in exactly here: implement [`JsEngine`] and pass it to
/// [`run_corpus_through_engine`]; the unchanged corpus holds it to byte-identical
/// behavior (`forge/spec/cross-engine-conformance.md` §5/§6).
///
/// `AdapterEngine` is a separate type (not `QuickJsEngine`) whose `run` forwards
/// through the public [`JsEngine`] trait to a `QuickJsEngine` it owns. It is a
/// faithful in-tree stand-in for "a different engine impl": the harness only ever
/// touches the trait object, so a passing run here means the seam — not a concrete
/// type — carries the conformance contract. Linking an actual foreign engine is
/// deferred infra (§6); this proves the harness is ready for it.
#[test]
fn second_engine_runs_the_same_corpus_byte_identically() {
    run_corpus_through_engine(&AdapterEngine::default(), "AdapterEngine");
}

#[test]
fn record_and_replay_use_the_injected_engine() {
    let engine = SentinelEngine;
    let manifest = conformance_manifest();
    let program = Program::new(
        "app_sentinel",
        "export async function main() { return { engine: 'quickjs' }; }",
    );
    let input = serde_json::json!({ "from": "test" });

    let mut bridge = MemoryHostBridge::new();
    let record = record_run_with_engine(
        &engine,
        &program,
        &manifest,
        &owner(),
        &input,
        7,
        11,
        &mut bridge,
    )
    .unwrap();
    assert_sentinel_record(&record, &program, &input);

    let mut null = NullBridge::new();
    let replayed = replay_with_engine(&engine, &record, &program, &manifest, &owner(), &mut null)
        .unwrap();
    assert_sentinel_record(&replayed, &program, &input);
    assert!(
        record.replays_identically(&replayed),
        "sentinel record and replay should be byte-identical"
    );
}

fn assert_sentinel_record(
    record: &forge_domain::RunRecord,
    program: &Program,
    input: &serde_json::Value,
) {
    match &record.outcome {
        RunOutcome::Completed { result } => {
            assert!(result.ok);
            assert_eq!(result.value["engine"], serde_json::json!("sentinel"));
            assert_eq!(result.value["code_hash"], serde_json::json!(program.code_hash()));
            assert_eq!(result.value["input"], *input);
        }
        other => panic!("sentinel engine should complete, got {other:?}"),
    }
}

struct SentinelEngine;

impl JsEngine for SentinelEngine {
    fn run(
        &self,
        program: &Program,
        input: &serde_json::Value,
        _host: &mut HostContext<'_>,
        _limits: &Limits,
    ) -> EngineOutcome {
        EngineOutcome {
            result: Ok(AppResult {
                ok: true,
                value: serde_json::json!({
                    "engine": "sentinel",
                    "code_hash": program.code_hash(),
                    "input": input,
                }),
            }),
            logs: vec!["sentinel-engine".to_string()],
        }
    }
}

/// A distinct `JsEngine` implementation used to prove the conformance harness is
/// engine-agnostic. It owns a `QuickJsEngine` and forwards `run` to it through the
/// **public trait** — so from the harness's perspective it is "a second engine":
/// a different Rust type reached only via `&dyn JsEngine`. A real foreign-engine
/// backend (JSC) would implement `JsEngine` the same way (marshalling
/// `serde_json::Value` in/out and forwarding `ctx.*` through the `HostContext`)
/// and be held to the identical corpus.
#[derive(Default)]
struct AdapterEngine {
    inner: QuickJsEngine,
}

impl JsEngine for AdapterEngine {
    fn run(
        &self,
        program: &Program,
        input: &serde_json::Value,
        host: &mut HostContext<'_>,
        limits: &Limits,
    ) -> EngineOutcome {
        self.inner.run(program, input, host, limits)
    }
}
