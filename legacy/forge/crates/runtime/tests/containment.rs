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

use common::{cpu_tight_manifest, mem_tight_manifest, owner, program, small_limits_manifest};
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
            r#"export async function main(c, i) { while (true) { await c.storage.get("app/flood"); } }"#
        }
        other => panic!("no JS equivalent wired for engine-owned corpus case {other:?}"),
    }
}

/// Every `suspended` / `runtime_error` corpus case is contained correctly and
/// quickly: the engine returns the expected `CoreError`, never hangs, never
/// panics. Small limits keep the whole thing well under a second.
#[test]
fn corpus_engine_owned_cases_are_contained() {
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

        // Split containment by category (review 020/021 P2). A memory-exhaustion
        // case run under a tight WALL could be "contained" by the wall interrupt
        // while a broken memory limiter does nothing — that would keep the suite
        // green for the wrong reason. So:
        //   * memory_exhaustion → GENEROUS wall/cpu, TIGHT memory ceiling: the
        //     memory limiter must be what wins, and we additionally assert the
        //     failure is classified as memory exhaustion (not the wall clock).
        //   * everything else (cpu_exhaustion / recursion / host_call_flood) →
        //     tight wall, where the wall/fuel/host-call budget is the fast limiter
        //     and the only assertion is the contained CoreError code.
        let is_memory = case.category == "memory_exhaustion";
        let manifest = if is_memory {
            mem_tight_manifest()
        } else {
            cpu_tight_manifest()
        };

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
            RunOutcome::Failed { error } => {
                assert_eq!(
                    error.code(),
                    expected_code,
                    "{} ({}) expected {} but got {} :: {error}",
                    case.file,
                    case.category,
                    expected_code,
                    error.code()
                );
                // Memory cases must fail THROUGH the memory limiter/classification,
                // not by the wall clock winning the race (review 020/021 P2). The
                // engine's `classify_failure` tags a memory/allocation exhaustion
                // as "memory budget exceeded ..."; a wall-clock trip reads
                // "CPU/wall-clock budget exceeded ...". Assert the former and
                // explicitly reject the latter so a regressed `set_memory_limit`
                // cannot pass on a wall interrupt.
                if is_memory {
                    let msg = error.to_string();
                    assert!(
                        msg.contains("memory budget exceeded"),
                        "{} ({}) must be contained by the MEMORY limiter, not the wall clock: {msg}",
                        case.file,
                        case.category
                    );
                    assert!(
                        !msg.contains("CPU/wall-clock"),
                        "{} ({}) tripped the wall clock instead of the memory ceiling: {msg}",
                        case.file,
                        case.category
                    );
                }
            }
            RunOutcome::Completed { result } => panic!(
                "{} ({}) should have been contained as {} but completed: {result:?}",
                case.file, case.category, expected_code
            ),
        }
        // Containment must be fast and never hang CI. CPU cases are bounded by the
        // 500ms wall; memory cases hit the small ceiling in a fraction of a second
        // (allocation growth is steep). The 5s ceiling is generous headroom for
        // realm build + the final interrupt interval under CPU contention — the
        // point is "does not hang CI", not a tight wall-clock assertion.
        assert!(
            elapsed.as_millis() < 5000,
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

/// CR-11/CR-12 determinism (T043): `Math.random` is NEUTRALIZED in the
/// deterministic realm. QuickJS seeds `Math.random` from system entropy at realm
/// creation — a non-seeded, non-recordable source that would break record/replay
/// determinism AND diverge from a second engine (JavaScriptCore). The realm
/// replaces it with a throwing stub, so the ONLY randomness is the seeded
/// `ctx.random.next()` seam. The property is still a function (`typeof` reports
/// `"function"`) but calling it throws a catchable `Error`, and it cannot be
/// redefined back to the entropy source. The deterministic Math functions
/// (`sqrt`/`round`/…) stay intact.
#[test]
fn math_random_is_neutralized_in_deterministic_mode() {
    let prog = program(
        r#"export async function main(ctx, input) {
            let threw = false, name = "none";
            try { Math.random(); } catch (e) { threw = true; name = e.name; }
            let reLockFailed = false;
            try {
                // Attempt to restore a live entropy source — must be refused
                // (the property is non-writable / non-configurable).
                Object.defineProperty(Math, "random", { value: () => 0.5 });
                Math.random();
            } catch (e) { reLockFailed = true; }
            return { ok: true, value: {
                typeofRandom: typeof Math.random,
                threw,
                name,
                reLockFailed,
                // Deterministic Math functions are untouched.
                sqrt2: Math.sqrt(2),
                round: Math.round(2.5),
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
    assert_eq!(value["typeofRandom"], serde_json::json!("function"));
    assert_eq!(value["threw"], serde_json::json!(true), "Math.random() must throw");
    assert_eq!(value["name"], serde_json::json!("Error"));
    assert_eq!(
        value["reLockFailed"],
        serde_json::json!(true),
        "Math.random must not be restorable to a live entropy source"
    );
    // The deterministic Math surface is unaffected by the neutralization.
    assert_eq!(value["sqrt2"], serde_json::json!(std::f64::consts::SQRT_2));
    assert_eq!(value["round"], serde_json::json!(3));
}

/// CR-11/CR-12 determinism (T043, review 180): the `Date` WALL-CLOCK readers are
/// NEUTRALIZED in the deterministic realm. `Date.now()`, zero-arg `new Date()`,
/// and `Date(...)` called as a function each read the host's wall-clock — a
/// non-seeded, non-recordable source that breaks record/replay determinism AND
/// diverges between engines. The realm replaces `Date` with a wrapper that THROWS
/// on those paths (mirroring `Math.random`) while PRESERVING argument-bearing,
/// fully-deterministic construction (`new Date(ms)`, ISO parsing) and the pure
/// statics (`Date.parse`/`Date.UTC`). The only clock is the seeded
/// `ctx.time.now()` seam. The global cannot be redefined back to wall-clock `Date`.
#[test]
fn date_wallclock_is_neutralized_in_deterministic_mode() {
    let prog = program(
        r#"export async function main(ctx, input) {
            function probe(fn) {
                try { fn(); return { threw: false, name: "none" }; }
                catch (e) { return { threw: true, name: e.name }; }
            }
            // Wall-clock readers must throw.
            const nowStatic = probe(() => Date.now());
            const zeroArgCtor = probe(() => new Date());
            const callAsFn = probe(() => Date());
            // Pure, deterministic constructions must survive.
            const fromMs = new Date(0).getTime();
            const fromIso = new Date("2020-01-02T03:04:05.000Z").getTime();
            const parsed = Date.parse("1970-01-01T00:00:00.000Z");
            const utc = Date.UTC(2020, 0, 1);
            const isInstance = (new Date(0)) instanceof Date;
            const iso = new Date(0).toISOString();
            // The global must not be restorable to the wall-clock Date.
            let reLockFailed = false;
            try {
                Object.defineProperty(globalThis, "Date", { value: function () { return 0; } });
                Date.now();
            } catch (e) { reLockFailed = true; }
            return { ok: true, value: {
                nowThrew: nowStatic.threw, nowName: nowStatic.name,
                zeroArgThrew: zeroArgCtor.threw,
                callAsFnThrew: callAsFn.threw,
                fromMs, fromIso, parsed, utc, isInstance, iso,
                reLockFailed,
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
    // Every wall-clock reader throws.
    assert_eq!(value["nowThrew"], serde_json::json!(true), "Date.now() must throw");
    assert_eq!(value["nowName"], serde_json::json!("Error"));
    assert_eq!(value["zeroArgThrew"], serde_json::json!(true), "new Date() must throw");
    assert_eq!(value["callAsFnThrew"], serde_json::json!(true), "Date() as a function must throw");
    // Deterministic constructions are preserved exactly.
    assert_eq!(value["fromMs"], serde_json::json!(0));
    assert_eq!(value["fromIso"], serde_json::json!(1_577_934_245_000i64));
    assert_eq!(value["parsed"], serde_json::json!(0));
    assert_eq!(value["utc"], serde_json::json!(1_577_836_800_000i64));
    assert_eq!(value["isInstance"], serde_json::json!(true), "instanceof Date must hold");
    assert_eq!(value["iso"], serde_json::json!("1970-01-01T00:00:00.000Z"));
    // The wall-clock Date cannot be restored.
    assert_eq!(
        value["reLockFailed"],
        serde_json::json!(true),
        "the global Date must not be restorable to a wall-clock source"
    );
}

/// The host-call flood guard (SC-2) trips before a tight `ctx.*` loop can run
/// away: `max_host_calls` is the cap and the run is suspended with
/// `ResourceLimitExceeded` after exactly that many calls.
#[test]
fn host_call_flood_is_capped_at_max_host_calls() {
    let mut manifest = small_limits_manifest();
    manifest.limits.max_host_calls = 50;
    // The host-call cap is the deterministic limiter here; the wall clock stays
    // the generous backstop from `small_limits_manifest` (30s) so CPU contention
    // can't trip the wall budget first and leave fewer than 51 recorded calls.
    let prog = program(
        r#"export async function main(ctx, input) {
            while (true) { await ctx.storage.get("app/flood"); }
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
    // Exactly `max_host_calls` calls succeeded, then the (n+1)th was DENIED by the
    // budget gate and that denial is now recorded too (review 009 P1 CR-9), so the
    // trace holds 50 successful calls + 1 recorded denial = 51 entries.
    assert_eq!(rec.calls.len(), 51);
    let denied = rec.calls.last().unwrap();
    assert_eq!(denied.method, "storage.get");
    assert!(
        denied.response.get("denied").is_some(),
        "the over-budget attempt must be recorded as a denial: {:?}",
        denied.response
    );
}

/// Review 009 P1 / CR-13: `eval` and `Function` are poisoned at the engine
/// level. After realm build `typeof eval === 'undefined'` and `typeof Function
/// === 'undefined'`, so dynamic code evaluation is unavailable regardless of the
/// static scan. This is the stronger engine guarantee that does not depend on
/// the pipeline scanner running.
#[test]
fn eval_and_function_globals_are_poisoned() {
    let prog = program(
        r#"export async function main(ctx, input) {
            return { ok: true, value: {
                eval: typeof eval,
                Function: typeof Function,
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
    match rec.outcome {
        RunOutcome::Completed { result } => {
            assert_eq!(result.value["eval"], serde_json::json!("undefined"), "eval must be poisoned");
            assert_eq!(
                result.value["Function"],
                serde_json::json!("undefined"),
                "Function constructor must be poisoned"
            );
        }
        other => panic!("typeof probe should complete, got {other:?}"),
    }
}

/// Calling the (now poisoned) `eval` is a plain runtime error — `undefined` is
/// not callable — never a path to dynamic code execution. The corpus
/// `eval_usage` case (`return eval("1 + 1")`) lands here at the engine level.
#[test]
fn calling_poisoned_eval_is_a_runtime_error_not_execution() {
    let prog = program(
        r#"export async function main(ctx, input) { return eval("1 + 1"); }"#,
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
        RunOutcome::Failed { error } => assert_eq!(error.code(), "RuntimeError"),
        other => panic!("calling poisoned eval must fail, not execute: {other:?}"),
    }
}

/// The corpus `function_constructor` case (`new Function(...)`) also fails at the
/// engine level now that `Function` is poisoned — `new undefined(...)` throws.
#[test]
fn new_function_constructor_is_a_runtime_error() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const make = new Function("return 1");
            return make();
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
        RunOutcome::Failed { error } => assert_eq!(error.code(), "RuntimeError"),
        other => panic!("new Function(...) must fail, not execute: {other:?}"),
    }
}

/// Review 019 P1 / CR-13: the `Function` constructor must not be reachable
/// through any function object's prototype chain. Nulling only the globals left
/// `(() => {}).constructor`, `(function(){}).constructor`, `(async
/// function(){}).constructor`, `(function*(){}).constructor` and `(async
/// function*(){}).constructor` all pointing at a live constructor. After the
/// fix, every such `constructor` is `undefined` for all function kinds.
#[test]
fn function_constructor_chain_is_poisoned_for_all_kinds() {
    let prog = program(
        r#"export async function main(ctx, input) {
            return { ok: true, value: {
                arrow:          typeof (() => {}).constructor,
                normal:         typeof (function () {}).constructor,
                async_fn:       typeof (async function () {}).constructor,
                generator:      typeof (function* () {}).constructor,
                async_gen:      typeof (async function* () {}).constructor,
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
    match rec.outcome {
        RunOutcome::Completed { result } => {
            for key in ["arrow", "normal", "async_fn", "generator", "async_gen"] {
                assert_eq!(
                    result.value[key],
                    serde_json::json!("undefined"),
                    "{key} .constructor must be poisoned (review 019 P1)"
                );
            }
        }
        other => panic!("constructor-chain probe should complete, got {other:?}"),
    }
}

/// Review 019 P1 / CR-13: the exact constructor-chain bypass the reviewer
/// confirmed against the global-only version — `(() => {}).constructor('return
/// 1+1')()` returned `2` — must now FAIL rather than execute dynamically. With
/// `constructor` poisoned to `undefined`, the call is `undefined('return 1+1')`,
/// which is a plain `RuntimeError`, never dynamic code execution.
#[test]
fn function_constructor_chain_bypass_does_not_execute() {
    for src in [
        // The reviewer's exact repro plus every other function kind, so the
        // chain bypass is proven unreachable at the ENGINE level for all of them
        // (review 020/021 P2): arrow, normal, async, generator, async-generator.
        r#"export async function main(ctx, input) { return (() => {}).constructor("return 1+1")(); }"#,
        r#"export async function main(ctx, input) { return (function () {}).constructor("return 1+1")(); }"#,
        r#"export async function main(ctx, input) { return (async function () {}).constructor("return 1+1")(); }"#,
        r#"export async function main(ctx, input) { return (function* () {}).constructor("return 1+1")(); }"#,
        r#"export async function main(ctx, input) { return (async function* () {}).constructor("return 1+1")(); }"#,
    ] {
        let prog = program(src);
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
            RunOutcome::Failed { error } => assert_eq!(
                error.code(),
                "RuntimeError",
                "constructor-chain bypass must fail, not execute: {src}"
            ),
            other => panic!("constructor-chain bypass must NOT execute ({src}): {other:?}"),
        }
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

/// Review 009 P2: a flood of *empty-string* `ctx.log("")` calls trips
/// `ResourceLimitExceeded` via the `max_host_calls` cap even though it adds zero
/// bytes — the log-byte budget alone can never stop it, so log calls are counted
/// against the host-call budget.
#[test]
fn empty_string_log_flood_trips_resource_limit() {
    let mut manifest = small_limits_manifest();
    manifest.limits.max_host_calls = 30;
    manifest.limits.log_bytes = 1024 * 1024; // huge: bytes are NOT the limiter
    // The log-call cap is the deterministic limiter; the wall clock stays the
    // generous backstop (30s, from `small_limits_manifest`) so contention can't
    // trip the wall budget first and surface a different error message than the
    // asserted "host-call" one.
    let prog = program(
        r#"export async function main(ctx, input) {
            while (true) { ctx.log(""); }
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
        other => panic!("expected ctx.log flood to trip host-call limit, got {other:?}"),
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
