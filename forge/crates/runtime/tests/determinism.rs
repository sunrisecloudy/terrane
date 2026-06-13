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

/// Review 032 enablement: the run API accepts caller-supplied `random_seed` /
/// `time_start` (it does not hard-default them), and replay re-uses the RECORDED
/// seeds. Two record runs with the **same** seeds produce identical seeded
/// values and replay byte-identically; two runs with **different** seeds produce
/// different seeded values. This is the contract forge-core threads to give every
/// `(applet, input)` a deterministic-but-distinct seed and the CLI scenario
/// fixtures rely on when running through the facade.
#[test]
fn run_api_threads_seeds_same_seeds_match_different_seeds_differ() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();
            const r = ctx.random.next();
            return { ok: true, value: { t, r } };
        }"#,
    );

    let run_with = |seed: u64, time_start: u64| {
        let mut bridge = MemoryHostBridge::new();
        record_run(
            &prog,
            &spine_manifest(),
            &owner(),
            &serde_json::json!({}),
            seed,
            time_start,
            &mut bridge,
        )
        .unwrap()
    };

    // Same seeds → the seeded time/random values (and the whole trace) match.
    let a = run_with(42, 1000);
    let b = run_with(42, 1000);
    assert_eq!(a.random_seed, 42);
    assert_eq!(a.time_start, 1000);
    assert_eq!(a.calls, b.calls, "same seeds must produce the same seeded trace");

    // Replay re-uses the RECORDED seeds (not any live default) and is identical.
    let mut null = NullBridge::new();
    let replayed = replay(&a, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    assert_eq!(replayed.random_seed, a.random_seed);
    assert_eq!(replayed.time_start, a.time_start);
    assert!(a.replays_identically(&replayed));

    // Different seeds → different seeded values. The random seam diverges on the
    // random_seed; the clock seam diverges on time_start.
    let c = run_with(43, 1000);
    let random_idx = a.calls.iter().position(|c| c.method == "random.next").unwrap();
    assert_ne!(
        a.calls[random_idx].response, c.calls[random_idx].response,
        "a different random_seed must yield a different ctx.random.next() value"
    );
    let d = run_with(42, 2000);
    let time_idx = a.calls.iter().position(|c| c.method == "time.now").unwrap();
    assert_ne!(
        a.calls[time_idx].response, d.calls[time_idx].response,
        "a different time_start must yield a different ctx.time.now() value"
    );
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

/// Review 026 P1: stripping `permissions` from a recorded *denial* must NOT let
/// it replay as a success. The manifest fallback (review 019 P2) is gated on a
/// *completed* outcome, so a failed record carrying the default snapshot — a
/// post-CR-9 denial with its `permissions` field removed by an old format or by
/// tampering — is replayed under the recorded all-deny snapshot, keeping the
/// denial denied even when the live manifest now grants the capability.
#[test]
fn stripping_permissions_from_a_recorded_denial_still_fails_on_replay() {
    use forge_domain::{PermissionSnapshot, RunRecord, StorageGrant};

    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("secret/x", 1); // outside grant -> PermissionDenied
            return { ok: true, value: "wrote" };
        }"#,
    );

    // Record under the default spine manifest: secret/* is NOT granted → the run
    // fails on a recorded denial.
    let mut bridge = MemoryHostBridge::new();
    let recorded = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(
        matches!(recorded.outcome, RunOutcome::Failed { .. }),
        "precondition: the run fails on the denied write"
    );
    assert_eq!(recorded.calls.len(), 1, "the denied call is recorded");
    assert!(
        recorded.calls[0].response.get("denied").is_some(),
        "precondition: the recorded response is a denial"
    );

    // Tamper: drop the `permissions` field, so the record loads with the all-deny
    // default snapshot — the same shape an attacker would use to try to re-grant
    // the capability from a permissive replay manifest.
    let mut json = serde_json::to_value(&recorded).unwrap();
    json.as_object_mut().unwrap().remove("permissions");
    let stripped: RunRecord = serde_json::from_value(json).unwrap();
    assert_eq!(stripped.permissions, PermissionSnapshot::default());
    assert!(!stripped.is_completed(), "the stripped record still failed");

    // Replay under a MORE PERMISSIVE manifest that DOES grant secret/*. Without
    // the review-026 gate, the completed-record manifest fallback would re-grant
    // the write and the run would succeed. It must still FAIL.
    let mut permissive = spine_manifest();
    permissive.capabilities.storage = StorageGrant {
        read: vec!["secret/*".into()],
        write: vec!["secret/*".into()],
    };
    let mut null = NullBridge::new();
    let replayed = replay(&stripped, &prog, &permissive, &owner(), &mut null).unwrap();
    match replayed.outcome {
        RunOutcome::Failed { error } => assert_eq!(
            error.code(),
            "PermissionDenied",
            "a stripped recorded denial must replay as a denial, not a success"
        ),
        other => panic!("stripped recorded denial must not replay as success, got {other:?}"),
    }
}

/// Review 029 P2: a snapshotless legacy run that made an **allowed** host call and
/// then *failed for an app/runtime reason* (not a policy denial) must replay its
/// recorded failure faithfully — it must NOT be turned into a different (spurious
/// permission) failure by the legacy fallback path.
///
/// The completed-only gate (review 026 P1) routed every snapshotless *failed*
/// legacy record through the all-deny default snapshot, so a run like
/// `await ctx.time.now(); throw new Error("boom")` would die at the first host
/// call with a `PermissionDenied` (the all-deny role gate) instead of replaying
/// the recorded `time.now` and reproducing the original `RuntimeError("boom")`.
/// The fix gates the manifest fallback on the recorded trace shape: a snapshotless
/// record with no recorded denial falls back to the manifest, so its successful
/// host calls replay and the original failure is reproduced byte-for-byte.
#[test]
fn snapshotless_legacy_failed_run_replays_its_recorded_failure_not_a_denial() {
    use forge_domain::{CoreError, PermissionSnapshot, RunRecord};

    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();          // an ALLOWED host call, recorded
            await ctx.storage.set("app/x", t); // a second ALLOWED host call
            throw new Error("boom");           // then fail for an app reason
        }"#,
    );

    // Record under the granting spine manifest: both host calls are allowed and
    // the run fails on the thrown error AFTER they are recorded.
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
    let recorded_error = match &recorded.outcome {
        RunOutcome::Failed { error } => error.clone(),
        other => panic!("precondition: the run must fail on the thrown error, got {other:?}"),
    };
    // The original failure is the app-level throw, surfaced as a RuntimeError that
    // carries the thrown message — NOT a permission denial.
    assert_eq!(recorded_error.code(), "RuntimeError", "{recorded_error}");
    assert!(recorded_error.to_string().contains("boom"), "{recorded_error}");
    // It made at least one successful host call BEFORE failing, and NONE of the
    // recorded calls is a denial (the precondition the fix keys on).
    assert!(recorded.calls.len() >= 2, "the allowed host calls are recorded");
    assert_eq!(recorded.calls[0].method, "time.now");
    assert!(
        recorded.calls.iter().all(|c| c.response.get("denied").is_none()),
        "no recorded call is a denial: {:?}",
        recorded.calls
    );

    // Simulate a PRE-CR-9 record: drop the `permissions` field so it loads with
    // the all-deny default snapshot (the legacy on-disk shape).
    let mut json = serde_json::to_value(&recorded).unwrap();
    json.as_object_mut().unwrap().remove("permissions");
    let legacy: RunRecord = serde_json::from_value(json).unwrap();
    assert_eq!(legacy.permissions, PermissionSnapshot::default());
    assert!(!legacy.is_completed(), "the legacy record still failed");

    // Replay under the granting manifest. The fallback must engage (no recorded
    // denial), so the allowed host calls replay and the ORIGINAL failure — the
    // RuntimeError("boom"), not a PermissionDenied — is reproduced.
    let mut null = NullBridge::new();
    let replayed = replay(&legacy, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    match &replayed.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(
                error.code(),
                "RuntimeError",
                "legacy failed run must replay its recorded failure, not a spurious denial: {error}"
            );
            assert!(error.to_string().contains("boom"), "{error}");
            assert_eq!(
                error, &recorded_error,
                "the replayed failure must be the recorded one"
            );
        }
        other => panic!("expected the recorded Runtime(\"boom\") failure, got {other:?}"),
    }
    // The recorded host-call trace replays identically (the allowed calls are
    // served from the recording, not re-denied under an all-deny snapshot).
    assert_eq!(
        recorded.calls, replayed.calls,
        "the legacy failed run's host-call trace must replay identically"
    );
    // Belt-and-suspenders: the replayed error is not a permission denial.
    assert!(
        !matches!(replayed.outcome, RunOutcome::Failed { error: CoreError::PermissionDenied(_) }),
        "the replay must NOT turn the app failure into a permission denial"
    );
}
