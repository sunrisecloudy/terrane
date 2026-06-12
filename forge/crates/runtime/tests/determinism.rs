//! Deterministic record/replay identity + divergence detection.
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-9 (run records), CR-11 (seeded
//! clock/RNG). This is the jewel's last link: "... → deterministic replay".

mod common;

use common::{owner, program, spine_manifest};
use forge_domain::RunOutcome;
use forge_runtime::{record_run, replay, MemoryHostBridge, NullBridge};

/// A program exercising the clock, RNG, and a storage write: record it, then
/// replay it and assert the replay is byte-identical to the original
/// (`replays_identically`, which excludes the per-invocation `run_id`).
#[test]
fn record_then_replay_is_identical() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();
            const r = ctx.random.next();
            await ctx.storage.set("app/state", { t, r, who: input.who });
            const got = await ctx.storage.get("app/state");
            await ctx.ui.render({ type: "text", value: got.who });
            ctx.log("recorded");
            return { ok: true, value: got };
        }"#,
    );

    let mut bridge = MemoryHostBridge::new();
    let original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({"who": "world"}),
        42,
        1000,
        &mut bridge,
    )
    .unwrap();
    assert!(original.is_completed());
    assert!(!original.calls.is_empty());

    // Replay against a NullBridge: the recorder must serve recorded responses
    // and never touch the live bridge (which refuses every effect).
    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap();

    assert!(
        original.replays_identically(&replayed),
        "replay must be byte-identical:\n original={:#?}\n replayed={:#?}",
        original.calls,
        replayed.calls
    );
    // The replay produced a (possibly) different run_id but the same trace.
    assert_eq!(original.calls, replayed.calls);
    assert_eq!(original.outcome, replayed.outcome);
}

/// Replaying twice yields the same fingerprint each time (stable determinism).
#[test]
fn replay_is_stable_across_repeats() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const r = ctx.random.next();
            await ctx.storage.set("app/r", r);
            return { ok: true, value: r };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        7,
        1,
        &mut bridge,
    )
    .unwrap();

    let mut n1 = NullBridge::new();
    let r1 = replay(&original, &prog, &spine_manifest(), &owner(), &mut n1).unwrap();
    let mut n2 = NullBridge::new();
    let r2 = replay(&original, &prog, &spine_manifest(), &owner(), &mut n2).unwrap();
    assert_eq!(r1.replay_fingerprint(), r2.replay_fingerprint());
    assert!(original.replays_identically(&r1));
}

/// Mutating a recorded response that a later call's arguments depend on makes
/// replay diverge with `RuntimeError` (the recorder catches the method/args
/// mismatch). The program reads a counter and writes `counter + 1` back, so a
/// tampered read response changes the subsequent write's args.
#[test]
fn mutating_a_recorded_response_diverges() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const cur = await ctx.storage.get("app/counter");
            const n = (cur === null ? 0 : cur) + 1;
            await ctx.storage.set("app/counter", n);
            return { ok: true, value: n };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    // The recorded read returned null; the write recorded args ["app/counter", 1].
    assert_eq!(original.calls[0].method, "storage.get");
    assert_eq!(
        original.calls[1].args,
        serde_json::json!(["app/counter", 1])
    );

    // Tamper with the read response so the live write computes a different value.
    let mut tampered = original.clone();
    tampered.calls[0].response = serde_json::json!(41);

    let mut null = NullBridge::new();
    let diverged = replay(&tampered, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    match diverged.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "RuntimeError");
            assert!(error.to_string().contains("divergence"), "{error}");
        }
        other => panic!("expected divergence RuntimeError, got {other:?}"),
    }
}

/// Replaying with a *different program* than was recorded is itself a
/// determinism error (the code hash guards it).
#[test]
fn replaying_different_code_is_rejected() {
    let prog_a =
        program("export async function main(ctx, input) { return { ok: true, value: 1 }; }");
    let prog_b =
        program("export async function main(ctx, input) { return { ok: true, value: 2 }; }");

    let mut bridge = MemoryHostBridge::new();
    let original = record_run(
        &prog_a,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();

    let mut null = NullBridge::new();
    let err = replay(&original, &prog_b, &spine_manifest(), &owner(), &mut null).unwrap_err();
    assert_eq!(err.code(), "RuntimeError");
    assert!(err.to_string().contains("code_hash"), "{err}");
}

/// A failed (suspended) run still records a replayable trace whose outcome
/// replays identically.
#[test]
fn failed_run_replays_identically() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("secret/x", 1); // outside grant -> PermissionDenied
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(matches!(original.outcome, RunOutcome::Failed { .. }));

    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    assert!(original.replays_identically(&replayed));
}
