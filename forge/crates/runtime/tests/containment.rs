//! Containment + limits, driven by the hostile corpus, plus the no-ambient
//! globals assertion.
//!
//! prd-merged/01 CR-1 (zero ambient), CR-5 (resource limits), CR-13 (two-layer
//! defense); prd-merged/07 SC-1/SC-2.
//!
//! Ownership split (CR-13): the corpus `manifest.json` marks each case with an
//! `expected_outcome`:
//!
//! - `suspended` (**owned here**) — a CPU/memory/host-call limit must trip
//!   (`ResourceLimitExceeded`) quickly, never hanging, never panicking across
//!   the FFI boundary.
//! - `runtime_error` (**owned here**) — recursion must normalize to
//!   `RuntimeError`, not a host stack overflow.
//! - `rejected_static` (**pipeline's job**) — `eval`/`Function`/forbidden-global
//!   /prototype-tamper cases belong to the static policy scanner (forge-pipeline,
//!   layer one). They are NOT run here; we assert only that *if* they reach the
//!   engine they do not escape containment (no panic, no ambient capability).
//!
//! The corpus files are TypeScript (the pipeline's SWC input); the runtime takes
//! plain JS, so for the `suspended`/`runtime_error` cases we run hand-written JS
//! equivalents keyed by corpus filename. The corpus `manifest.json` drives which
//! cases exist and what each must do, so the corpus stays the source of truth.

mod common;

use common::{owner, program, small_limits_manifest};
use forge_domain::RunOutcome;
use forge_runtime::{record_run, MemoryHostBridge};
use std::time::Instant;

/// One corpus case as declared in `tests/corpus/manifest.json`.
#[derive(serde::Deserialize)]
struct CorpusCase {
    file: String,
    category: String,
    expected_outcome: String,
    expected_error: String,
}

#[derive(serde::Deserialize)]
struct CorpusManifest {
    cases: Vec<CorpusCase>,
}

fn load_corpus() -> CorpusManifest {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/manifest.json");
    let text = std::fs::read_to_string(path).expect("read corpus manifest");
    serde_json::from_str(&text).expect("parse corpus manifest")
}

/// Hand-written JS equivalents of the hostile corpus cases the *engine* owns
/// (the `suspended` / `runtime_error` rows). Keyed by corpus filename so the
/// corpus manifest remains the selection driver.
fn js_for(file: &str) -> &'static str {
    match file {
        "infinite_loop.ts" => "export async function main(c, i) { while (true) {} }",
        "deep_for_nesting.ts" => {
            "export async function main(c, i) { let t = 0; for (let a = 0; a < 1000000; a++) { for (let b = 0; b < 1000000; b++) { t += a + b; } } return t; }"
        }
        "catastrophic_regex.ts" => {
            r#"export async function main(c, i) { return /^(a+)+$/.test("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa!"); }"#
        }
        "unbounded_array_push.ts" => {
            r#"export async function main(c, i) { const v = []; while (true) { v.push("x".repeat(1024)); } }"#
        }
        "huge_string_concat.ts" => {
            r#"export async function main(c, i) { let v = "x"; while (true) { v += v; } }"#
        }
        "deep_object_nesting.ts" => {
            "export async function main(c, i) { let n = {}; const r = n; while (true) { n.child = {}; n = n.child; } return r; }"
        }
        "direct_recursion.ts" => {
            "function recurse(d) { return recurse(d + 1); } export async function main(c, i) { return recurse(0); }"
        }
        "mutual_recursion.ts" => {
            "function left(d) { return right(d + 1); } function right(d) { return left(d + 1); } export async function main(c, i) { return left(0); }"
        }
        "ctx_storage_flood.ts" => {
            r#"export async function main(c, i) { while (true) { await c.storage.get("flood"); } }"#
        }
        other => panic!("no JS equivalent wired for engine-owned corpus case {other:?}"),
    }
}

/// Every `suspended` / `runtime_error` corpus case is contained correctly and
/// quickly: the engine returns the expected `CoreError`, never hangs, never
/// panics. Small limits keep the whole thing well under a second.
#[test]
fn corpus_engine_owned_cases_are_contained() {
    let manifest = small_limits_manifest();
    let corpus = load_corpus();
    let mut checked = 0;

    for case in &corpus.cases {
        let expected_code = match case.expected_outcome.as_str() {
            "suspended" => "ResourceLimitExceeded",
            "runtime_error" => "RuntimeError",
            // Static-scan cases are the pipeline crate's job; skipped here (see
            // the separate assertion below that they at least don't escape).
            "rejected_static" => continue,
            other => panic!("unknown expected_outcome {other:?} for {}", case.file),
        };
        assert_eq!(
            case.expected_error, expected_code,
            "corpus row {} declares expected_error {} but expected_outcome {} implies {}",
            case.file, case.expected_error, case.expected_outcome, expected_code
        );

        let prog = program(js_for(&case.file));
        let mut bridge = MemoryHostBridge::new();
        let start = Instant::now();
        let rec = record_run(
            &prog,
            &manifest,
            &owner(),
            &serde_json::json!({}),
            1,
            0,
            &mut bridge,
        )
        .unwrap();
        let elapsed = start.elapsed();

        match rec.outcome {
            RunOutcome::Failed { error } => assert_eq!(
                error.code(),
                expected_code,
                "{} ({}) expected {} but got {} :: {error}",
                case.file,
                case.category,
                expected_code,
                error.code()
            ),
            RunOutcome::Completed { result } => panic!(
                "{} ({}) should have been contained as {} but completed: {result:?}",
                case.file, case.category, expected_code
            ),
        }
        // Containment must be fast — the budgets are small; well under a second.
        assert!(
            elapsed.as_millis() < 1500,
            "{} took too long ({elapsed:?}); containment must not hang CI",
            case.file
        );
        checked += 1;
    }

    // Sanity: we actually exercised the engine-owned cases the corpus declares.
    let owned = corpus
        .cases
        .iter()
        .filter(|c| c.expected_outcome == "suspended" || c.expected_outcome == "runtime_error")
        .count();
    assert_eq!(
        checked, owned,
        "every engine-owned corpus case must be checked"
    );
    assert!(
        checked >= 8,
        "expected the full hostile corpus, only {checked} cases"
    );
}

/// CR-1 / containment: the realm exposes **no ambient capability globals**. A
/// script probing `fetch`/`process`/`require`/`XMLHttpRequest`/`global` sees
/// `undefined` for all of them — there is no escape hatch beyond `ctx`.
#[test]
fn realm_has_no_ambient_capability_globals() {
    let prog = program(
        r#"export async function main(ctx, input) {
            return { ok: true, value: {
                fetch: typeof fetch,
                process: typeof process,
                require: typeof require,
                xhr: typeof XMLHttpRequest,
                global: typeof global,
                module: typeof module,
                Deno: typeof Deno,
            } };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &small_limits_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    let value = match rec.outcome {
        RunOutcome::Completed { result } => result.value,
        other => panic!("probe should complete, got {other:?}"),
    };
    for key in [
        "fetch", "process", "require", "xhr", "global", "module", "Deno",
    ] {
        assert_eq!(
            value[key],
            serde_json::json!("undefined"),
            "ambient global {key:?} must not exist in the realm (got {})",
            value[key]
        );
    }
    // `ctx` IS present (the one intentional host object).
    let prog2 = program(
        "export async function main(ctx, input) { return { ok: true, value: typeof ctx }; }",
    );
    let mut b2 = MemoryHostBridge::new();
    let r2 = record_run(
        &prog2,
        &small_limits_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut b2,
    )
    .unwrap();
    assert_eq!(
        r2.outcome,
        RunOutcome::Completed {
            result: forge_domain::AppResult {
                ok: true,
                value: serde_json::json!("object")
            }
        }
    );
}

/// The host-call flood guard (SC-2) trips before a tight `ctx.*` loop can run
/// away: `max_host_calls` is the cap and the run is suspended with
/// `ResourceLimitExceeded` after exactly that many calls.
#[test]
fn host_call_flood_is_capped_at_max_host_calls() {
    let mut manifest = small_limits_manifest();
    manifest.limits.max_host_calls = 50;
    manifest.limits.wall_ms = 2000; // give the loop room so the CALL cap wins
    let prog = program(
        r#"export async function main(ctx, input) {
            while (true) { await ctx.storage.get("flood"); }
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &manifest,
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "ResourceLimitExceeded");
            assert!(error.to_string().contains("host-call"), "{error}");
        }
        other => panic!("expected host-call limit, got {other:?}"),
    }
    // Exactly `max_host_calls` calls were recorded before the cap tripped.
    assert_eq!(rec.calls.len(), 50);
}

/// The `eval` static-scan case is the pipeline scanner's responsibility, but if
/// such code reached the engine it must not escape containment. We assert the
/// stronger property the engine guarantees regardless of the scanner: even with
/// `eval` available (a known QuickJS capability, CR-13), the realm still has no
/// ambient host capability, so `eval` cannot reach `fetch`/`process`/etc.
#[test]
fn eval_cannot_reach_ambient_capability_even_if_unscanned() {
    let prog = program(
        r#"export async function main(ctx, input) {
            // eval is intentionally present (layer-2 defense relies on the static
            // scan to reject it); prove it still yields no ambient capability.
            const probe = eval("typeof fetch + ',' + typeof process + ',' + typeof require");
            return { ok: true, value: probe };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &small_limits_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Completed { result } => {
            assert_eq!(
                result.value,
                serde_json::json!("undefined,undefined,undefined")
            );
        }
        other => panic!("eval probe should complete without escaping, got {other:?}"),
    }
}

/// The storage byte budget (CR-5) suspends a run that writes more than
/// `storage_bytes` total, independent of the host-call count.
#[test]
fn storage_byte_budget_is_enforced() {
    let mut manifest = small_limits_manifest();
    manifest.limits.storage_bytes = 256; // tiny
    manifest.limits.max_host_calls = 10_000; // not the limiting factor
    let prog = program(
        r#"export async function main(ctx, input) {
            const big = "x".repeat(1000);
            await ctx.storage.set("app/a", big);
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &manifest,
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "ResourceLimitExceeded");
            assert!(error.to_string().contains("storage byte"), "{error}");
        }
        other => panic!("expected storage byte budget suspension, got {other:?}"),
    }
}

/// The log byte budget (CR-5) suspends a run that logs more than `log_bytes`.
#[test]
fn log_byte_budget_is_enforced() {
    let mut manifest = small_limits_manifest();
    manifest.limits.log_bytes = 16; // tiny
    let prog = program(
        r#"export async function main(ctx, input) {
            ctx.log("this line is definitely longer than sixteen bytes");
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &manifest,
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "ResourceLimitExceeded");
            assert!(error.to_string().contains("log byte"), "{error}");
        }
        other => panic!("expected log byte budget suspension, got {other:?}"),
    }
}
