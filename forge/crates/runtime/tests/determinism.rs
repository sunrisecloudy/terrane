//! Deterministic record/replay identity + divergence detection.
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-9 (run records), CR-11 (seeded
//! clock/RNG). This is the jewel's last link: "... → deterministic replay".

mod common;

use common::{owner, program, spine_manifest};
use forge_domain::{RecordedCall, RunOutcome};
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

/// Reviews 012/013/014: the runtime records the canonical `sha256:` provenance
/// hash and never the old `fnv1a64:` form; the recorded hash passes the domain's
/// canonical-hash validator, so a record this engine emits can never carry a
/// divergent provenance string.
#[test]
fn runtime_emits_canonical_hash_never_fnv1a64() {
    let prog =
        program("export async function main(ctx, input) { return { ok: true, value: 1 }; }");
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(rec.code_hash.starts_with("sha256:"), "got {}", rec.code_hash);
    assert!(!rec.code_hash.starts_with("fnv1a64:"));
    assert!(forge_domain::is_canonical_code_hash(&rec.code_hash));
    // The record validates (RunRecord::new validated it at construction).
    assert!(rec.validate_code_hash().is_ok());
}

/// Reviews 012/013/014 (the teeth): replay REFUSES a record whose `code_hash` is
/// the old non-canonical `fnv1a64:` form — even before comparing it to the
/// program — so a forged/legacy record cannot be replayed as if valid.
#[test]
fn replay_rejects_a_record_with_fnv1a64_code_hash() {
    let prog =
        program("export async function main(ctx, input) { return { ok: true, value: 1 }; }");
    let mut bridge = MemoryHostBridge::new();
    let mut original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    // Forge a non-canonical provenance string onto the record.
    original.code_hash = "fnv1a64:0123456789abcdef".into();
    let mut null = NullBridge::new();
    let err = replay(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap_err();
    assert_eq!(err.code(), "ValidationError", "non-canonical hash must be rejected: {err}");
}

/// Review 012 P2: the derived `run_id` is built from the digest *body*, not the
/// `alg:` prefix. Under `sha256:` the id reads from the hash digest, so it never
/// contains the literal `"sha256"` algorithm tag (the bug the old
/// `trim_start_matches("fnv1a64:")` left once the algorithm changed).
#[test]
fn run_id_is_digest_based_and_algorithm_agnostic() {
    let prog =
        program("export async function main(ctx, input) { return { ok: true, value: 1 }; }");
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        7,
        3,
        &mut bridge,
    )
    .unwrap();
    let id = rec.run_id.to_string();
    // The id embeds the first 8 hex chars of the sha256 digest body.
    let digest = rec.code_hash.strip_prefix("sha256:").unwrap();
    assert!(
        id.contains(&digest[..8]),
        "run_id {id} must embed the digest prefix {}",
        &digest[..8]
    );
    // It must NOT have leaked the algorithm tag into the id.
    assert!(!id.contains("sha256"), "run_id must be digest-based, not contain the alg tag: {id}");
    assert!(id.starts_with("run_"));
}

/// Review 009 P2: replay must FAIL if not every recorded call is consumed.
/// Appending one extra recorded call to an otherwise valid trace makes the
/// replay end with a leftover recorded call → `RuntimeError` divergence.
#[test]
fn replay_fails_when_recorded_calls_are_left_unconsumed() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();
            return { ok: true, value: t };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let mut original = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(original.is_completed());
    let next_seq = original.calls.len() as u64;
    // Append one EXTRA recorded call the live run will never issue.
    original.calls.push(RecordedCall {
        seq: next_seq,
        method: "time.now".into(),
        args: serde_json::Value::Null,
        response: serde_json::json!(999),
    });

    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    match replayed.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "RuntimeError");
            assert!(error.to_string().contains("unconsumed"), "{error}");
        }
        other => panic!("expected unconsumed-calls divergence, got {other:?}"),
    }
}

/// Review 009 P1 (CR-9): a denied host call is recorded as a deterministic
/// denial and replays identically. The program attempts a write outside its
/// grant; the denial is now in the trace (not vanished), so record→replay is
/// byte-identical.
#[test]
fn denied_host_call_is_recorded_and_replays_identically() {
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
    // The denied attempt is recorded (it used to vanish before recorder.host_call).
    assert_eq!(original.calls.len(), 1, "the denied call must be recorded");
    assert_eq!(original.calls[0].method, "storage.set");
    assert!(
        original.calls[0].response.get("denied").is_some(),
        "denial response must capture the error: {:?}",
        original.calls[0].response
    );

    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    assert!(original.replays_identically(&replayed));
}

/// Review 009 P1 (CR-9): replay uses the RECORDED permission snapshot, not the
/// live manifest. A run recorded WITHOUT a `secret/*` grant (the write is denied)
/// must still replay as a denial even when replayed under a manifest that DOES
/// grant `secret/*` — the recorded decision is authoritative.
#[test]
fn replay_uses_recorded_permission_snapshot_not_current_grants() {
    use forge_domain::StorageGrant;

    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("secret/x", 1);
            return { ok: true, value: "wrote" };
        }"#,
    );

    // Record under the default spine manifest: secret/* is NOT granted → denied.
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
    // Sanity: the snapshot we recorded does NOT grant secret/*.
    assert!(!original.permissions.capabilities.storage.write.iter().any(|s| s == "secret/*"));

    // Now replay under a MORE PERMISSIVE manifest that DOES grant secret/*.
    // (A bare `*` grant is rejected as overly broad by forge-policy review 006
    // P2; a scoped `secret/*` grant is enough to make the point that the live
    // manifest is permissive while the recorded snapshot — which lacks it — is
    // what actually governs the replay decision.)
    let mut permissive = spine_manifest();
    permissive.capabilities.storage = StorageGrant {
        read: vec!["secret/*".into()],
        write: vec!["secret/*".into()],
    };
    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &permissive, &owner(), &mut null).unwrap();

    // The replay must reproduce the RECORDED denial, not succeed under the new
    // grant — and it must be byte-identical to the original.
    assert!(
        original.replays_identically(&replayed),
        "replay must honor the recorded snapshot, not the live (permissive) manifest"
    );
    match replayed.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("replay should reproduce the recorded denial, got {other:?}"),
    }
}

/// Review 019 P2: a legacy/pre-CR-9 record carries no permission snapshot, so it
/// deserializes with the all-deny `PermissionSnapshot::default()` (`can_run =
/// false`, `max_host_calls = 0`, no grants). Replay must NOT treat that absent
/// snapshot as an explicit all-deny — a legitimate historical run that did
/// `time`/`random`/`storage`/`ui`/`log` host calls must still replay correctly,
/// falling back to the caller-provided manifest/actor. Without the fix, the
/// snapshot-less record replays as a permission/resource denial.
#[test]
fn snapshotless_legacy_record_replays_against_manifest_not_all_deny() {
    use forge_domain::{PermissionSnapshot, RunRecord};

    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();
            const r = ctx.random.next();
            await ctx.storage.set("app/state", { t, r });
            const got = await ctx.storage.get("app/state");
            await ctx.ui.render({ type: "text", value: "ok" });
            ctx.log("legacy");
            return { ok: true, value: got };
        }"#,
    );

    // Record a normal, completing run (it makes several host calls).
    let mut bridge = MemoryHostBridge::new();
    let recorded = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        7,
        500,
        &mut bridge,
    )
    .unwrap();
    assert!(recorded.is_completed(), "precondition: the run completes");
    assert!(!recorded.calls.is_empty(), "precondition: it made host calls");

    // Simulate a PRE-CR-9 record: serialize, drop the `permissions` field, and
    // deserialize. The missing field defaults to the all-deny snapshot — exactly
    // what an old on-disk record would load as (mirrors the domain test
    // `missing_permission_snapshot_defaults_on_deserialize`).
    let mut json = serde_json::to_value(&recorded).unwrap();
    json.as_object_mut().unwrap().remove("permissions");
    let legacy: RunRecord = serde_json::from_value(json).unwrap();
    assert_eq!(
        legacy.permissions,
        PermissionSnapshot::default(),
        "a snapshot-less record loads as the all-deny default"
    );

    // Replay the legacy record under the granting manifest/actor. It must
    // complete (fall back to the manifest), NOT fail as an all-deny denial.
    let mut null = NullBridge::new();
    let replayed = replay(&legacy, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    match &replayed.outcome {
        RunOutcome::Completed { .. } => {}
        RunOutcome::Failed { error } => panic!(
            "legacy snapshot-less record must replay against the manifest, not as all-deny: {error}"
        ),
    }
    // And the trace it produces matches the recorded one (the host calls replay).
    assert_eq!(
        recorded.calls, replayed.calls,
        "the legacy record's host-call trace must replay identically"
    );
}
