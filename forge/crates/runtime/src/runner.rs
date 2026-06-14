//! Record/replay orchestration: turn an engine run into a [`RunRecord`].
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-9 (run records), CR-11 (seams).
//! This is the public M0a API the spine wires through `forge-core`:
//!
//!   * [`record_run`] runs a [`Program`] in record mode and produces a
//!     [`RunRecord`] capturing the seeded seams + ordered host-call trace +
//!     outcome.
//!   * [`replay`] re-runs the *same* program against a recorded [`RunRecord`],
//!     serving recorded responses (the live bridge is never consulted for
//!     reads/seams), and produces a fresh `RunRecord` that must
//!     [`replays_identically`](RunRecord::replays_identically) to the original.
//!
//! Native only (drives [`QuickJsEngine`]); gated alongside the engine.

use crate::bridge::HostBridge;
use crate::engine::QuickJsEngine;
use crate::host::HostContext;
use crate::recorder::RunRecorder;
use crate::{EngineOutcome, JsEngine, Program};
use forge_domain::{
    ActorContext, AppResult, CoreError, Manifest, PermissionSnapshot, Result, RunId, RunOutcome,
    RunRecord,
};
use forge_policy::PolicyEngine;

/// Run `program` under `manifest` in **record mode** and produce a
/// [`RunRecord`]. `actor` gates capabilities (owner-permits-all in M0a);
/// `seed`/`time_start` seed the deterministic RNG/clock seams; `bridge` provides
/// the live effects (storage/db/ui/log) that are captured into the trace.
///
/// The returned record's `run_id` is derived from the code hash + seed so two
/// record runs of the same program/seed are stable yet distinguishable per
/// invocation via the caller; callers that persist runs typically overwrite by
/// id (see `Store::save_run`).
pub fn record_run(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    input: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    let recorder = RunRecorder::recording(seed, time_start);
    // `PolicyEngine::new` now validates the manifest's storage glob grants
    // (forge-policy review 006 P2); a bare `*`/malformed grant fails closed here
    // as a ValidationError rather than being silently accepted.
    let policy = PolicyEngine::new(manifest, actor)?;
    finish_run(
        program,
        policy,
        manifest.limits.clone(),
        input,
        seed,
        time_start,
        recorder,
        bridge,
    )
}

/// Dispatch a UI event to the applet's handler named `action_ref` in **record
/// mode**, producing a [`RunRecord`] (prd-merged/05 UI-4, prd-merged/01 CR-6).
///
/// This is the record-side of the interactive loop: the rendered tree carried an
/// `onTap`/`onChange` `ActionRef`, the renderer sent that ref back with an event
/// `payload`, and this drives the handler exported under that name over the same
/// containment / limits / host path as [`record_run`]. The handler's `ctx.*`
/// effects are recorded as usual, plus a `ui.dispatch_event` envelope capturing
/// `(action_ref, payload) -> result`, so the event replays byte-identically via
/// [`replay_dispatch`].
///
/// State persists ONLY through `ctx.db`/`ctx.storage`: the realm is one-shot per
/// dispatch (a fresh realm per call), so a handler that needs to see a prior
/// dispatch's effect must have written it through the host bridge. An unknown
/// `action_ref` makes the run fail with a typed `ValidationError` (no such
/// handler), recorded as the run's outcome — never a panic.
#[allow(clippy::too_many_arguments)]
pub fn record_dispatch(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    action_ref: &str,
    payload: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    let recorder = RunRecorder::recording(seed, time_start);
    let policy = PolicyEngine::new(manifest, actor)?;
    finish_dispatch(
        program,
        policy,
        manifest.limits.clone(),
        action_ref,
        payload,
        seed,
        time_start,
        recorder,
        bridge,
    )
}

/// Replay a recorded UI event dispatch (the counterpart to [`record_dispatch`]).
/// Re-runs the same handler in **replay mode**: the recorder serves the recorded
/// `ctx.*` responses and asserts the dispatched `(action_ref, payload)` matches
/// the recording, so the produced record must
/// [`replays_identically`](RunRecord::replays_identically) to `run`. A diverging
/// event (different action ref, payload, or order) is a determinism
/// `RuntimeError`. The recorded permission snapshot governs the replay decision
/// exactly as in [`replay`] (CR-9); the pre-CR-9 manifest fallback applies too.
pub fn replay_dispatch(
    run: &RunRecord,
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    run.validate_code_hash()?;
    if program.code_hash() != run.code_hash {
        return Err(CoreError::RuntimeError(format!(
            "determinism divergence: replay program code_hash {} != recorded {}",
            program.code_hash(),
            run.code_hash
        )));
    }
    // Recover the dispatched `(action_ref, payload)` from the recorded
    // `ui.dispatch_event` envelope so the replay re-issues the SAME event. A
    // record produced by `record_dispatch` always carries exactly one such entry.
    let (action_ref, payload) = recorded_dispatch(run)?;
    let recorder = RunRecorder::replaying(run.random_seed, run.time_start, run.calls.clone());
    let (policy, host_call_cap) = replay_policy(run, manifest, actor)?;
    let mut limits = manifest.limits.clone();
    limits.max_host_calls = host_call_cap;
    finish_dispatch(
        program,
        policy,
        limits,
        &action_ref,
        &payload,
        run.random_seed,
        run.time_start,
        recorder,
        bridge,
    )
}

/// Recover the `(action_ref, payload)` of the dispatched UI event from a recorded
/// run's `ui.dispatch_event` envelope (its `args = [action_ref, payload]`). A
/// record built by [`record_dispatch`] carries exactly one such call; its absence
/// (or a malformed shape) means the record is not a dispatch record, which is a
/// `RuntimeError` rather than a silent wrong-event replay.
fn recorded_dispatch(run: &RunRecord) -> Result<(String, serde_json::Value)> {
    let call = run
        .calls
        .iter()
        .find(|c| c.method == "ui.dispatch_event")
        .ok_or_else(|| {
            CoreError::RuntimeError(
                "replay_dispatch: record carries no ui.dispatch_event envelope".into(),
            )
        })?;
    match call.args.as_array().map(|a| a.as_slice()) {
        Some([action_ref, payload]) => {
            let action_ref = action_ref.as_str().ok_or_else(|| {
                CoreError::RuntimeError(
                    "replay_dispatch: recorded action_ref is not a string".into(),
                )
            })?;
            Ok((action_ref.to_string(), payload.clone()))
        }
        _ => Err(CoreError::RuntimeError(
            "replay_dispatch: ui.dispatch_event args are not [action_ref, payload]".into(),
        )),
    }
}

/// Replay a previously recorded [`RunRecord`] by re-running `program` in
/// **replay mode**. The recorder serves the recorded responses and detects any
/// divergence (→ `RuntimeError`). `bridge` should be a
/// [`NullBridge`](crate::NullBridge) (or any bridge): replay never touches live
/// reads/seams, so the bridge is only a safety net.
///
/// The produced record must `replays_identically` to `run`; callers/tests
/// assert this to prove determinism.
///
/// `actor` is consulted **only** for a legacy/pre-CR-9 record that carries no
/// permission snapshot (see below). For any record produced by this engine since
/// CR-9, the permission decision on replay comes from the record's
/// [`PermissionSnapshot`](forge_domain::PermissionSnapshot), not the live
/// actor/manifest (review 009 P1 CR-9), so a replay is governed by the
/// permissions it was recorded under.
///
/// **Pre-CR-9 fallback (review 019 P2, tightened in review 026).** Older records
/// predate the permission snapshot: deserializing them defaults `permissions` to
/// [`PermissionSnapshot::default`], which is the *all-deny* state (`can_run =
/// false`, `max_host_calls = 0`, no grants). Replaying such a record against that
/// default would turn a legitimate historical run — one that did `time`/`random`/
/// storage/db/ui/log calls — into a spurious permission/resource denial, even
/// though the run had completed cleanly when recorded. We refuse to treat
/// "snapshot absent" as "explicitly all-deny": when `run.permissions` is exactly
/// the default snapshot **and the record completed**, we fall back to building the
/// replay policy from the *caller-provided* manifest/actor (the pre-CR-9 replay
/// behavior) rather than denying everything.
///
/// The fallback is gated on the **recorded trace shape**, not the outcome (review
/// 029 P2, tightening review 026 P1). A record can have its `permissions` field
/// stripped — by an old format or by tampering — and still load, so an attacker
/// could take a post-CR-9 run that *failed* on a recorded denial (e.g. a denied
/// `storage.set`), remove `permissions`, and replay it under a now-permissive
/// manifest. Falling back to the live manifest there would re-grant the very
/// capability the original lacked and turn the recorded denial into a success.
/// The denial-specific signal already lives in the trace: a policy denial is
/// recorded by [`RunRecorder::record_denial`](crate::RunRecorder) as a
/// `{"denied": <CoreError>}` response. So we extend the manifest fallback to any
/// snapshotless record whose recorded calls contain **no recorded denial**, and
/// keep a record that *does* carry a recorded denial on the recorded (all-deny
/// default) snapshot path ([`from_snapshot`](PolicyEngine::from_snapshot)).
///
/// This is strictly more faithful than the prior completed-only gate. A genuine
/// pre-CR-9 run that made an *allowed* host call and then failed for an
/// app/runtime reason (`await ctx.time.now(); throw new Error("boom")`) carries no
/// recorded denial, so it falls back to the manifest and replays its recorded
/// success-then-failure instead of dying at the first host call under the
/// all-deny default. A stripped post-CR-9 denial still carries its `{"denied": …}`
/// call, so it stays on the all-deny path and a recorded denial stays denied — a
/// stripped failure cannot replay as a success. A record produced post-CR-9 always
/// carries a real snapshot, so this fallback never masks a genuine all-deny replay.
pub fn replay(
    run: &RunRecord,
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    // The recorded run's provenance hash must itself be canonical (reviews
    // 012/013/014): a record carrying a non-canonical `code_hash` (e.g. the old
    // `fnv1a64:` string) is not a record this engine can trust to replay.
    run.validate_code_hash()?;
    // Guard: replaying different code than was recorded is a determinism error.
    // `program.code_hash()` is now the canonical `sha256:` form, so this also
    // refuses a stale record whose hash uses a different algorithm.
    if program.code_hash() != run.code_hash {
        return Err(CoreError::RuntimeError(format!(
            "determinism divergence: replay program code_hash {} != recorded {}",
            program.code_hash(),
            run.code_hash
        )));
    }
    let recorder = RunRecorder::replaying(run.random_seed, run.time_start, run.calls.clone());
    let (policy, host_call_cap) = replay_policy(run, manifest, actor)?;
    let mut limits = manifest.limits.clone();
    limits.max_host_calls = host_call_cap;
    finish_run(
        program,
        policy,
        limits,
        &run.input,
        run.random_seed,
        run.time_start,
        recorder,
        bridge,
    )
}

/// Select the replay-mode policy + host-call cap for a recorded run (CR-9), shared
/// by [`replay`] and [`replay_dispatch`].
///
/// CR-9 (review 009 P1): build the replay policy from the RECORDED permission
/// snapshot, not the live manifest/actor, so a call denied (or allowed) at record
/// time replays with the same decision even if the workspace's grants, role, or
/// budget have since changed. Engine-level limits (memory/fuel/wall) still come
/// from the manifest, but the host-call cap tracks the snapshot so the budget gate
/// behaves identically on replay.
///
/// Exception — review 019 P2 / 029 P2: a pre-CR-9 record has no snapshot, which
/// deserializes to the all-deny default. Don't replay a legitimate historical run
/// as an all-deny denial; fall back to the caller-provided manifest/actor (the
/// legacy replay path) and use the manifest's host-call cap — but ONLY when no
/// recorded call is a denial. A stripped post-CR-9 denial keeps its `{"denied": …}`
/// call and stays on the recorded all-deny snapshot, so a recorded denial cannot
/// replay as a success.
fn replay_policy(
    run: &RunRecord,
    manifest: &Manifest,
    actor: &ActorContext,
) -> Result<(PolicyEngine, u64)> {
    let use_manifest_fallback =
        run.permissions == PermissionSnapshot::default() && !trace_has_denial(&run.calls);
    if use_manifest_fallback {
        Ok((
            PolicyEngine::new(manifest, actor)?,
            manifest.limits.max_host_calls,
        ))
    } else {
        Ok((
            PolicyEngine::from_snapshot(&run.permissions)?,
            run.permissions.max_host_calls,
        ))
    }
}

/// Shared body for record/replay: drive the engine with a prepared recorder +
/// policy and assemble the [`RunRecord`].
///
/// The record is built through [`RunRecord::new`] (which **validates** the
/// `code_hash`) rather than a struct literal, so a non-canonical provenance hash
/// can never be emitted (reviews 012/013/014). The evaluated permission snapshot
/// is attached (review 009 P1 CR-9), and on replay the recorder is asserted to
/// have consumed every recorded call (review 009 P2).
/// How [`finish_run`] should drive the engine for this run record: the program's
/// `main(ctx, input)`, or a UI event handler addressed by `ActionRef` with a
/// payload (UI-4/CR-6). The two share the entire record/replay assembly below;
/// they differ only in which engine entrypoint runs and (for the handler) the
/// extra `ui.dispatch_event` envelope recorded around the handler's effects.
enum Drive<'a> {
    /// Drive `main(ctx, input)` (the classic run/replay path).
    Main { input: &'a serde_json::Value },
    /// Dispatch `<action_ref>(ctx, payload)` and record the dispatch envelope so
    /// the event replays identically.
    Handler {
        action_ref: &'a str,
        payload: &'a serde_json::Value,
    },
}

impl<'a> Drive<'a> {
    /// The value recorded as the run record's `input` (the `main` input, or the
    /// dispatched event's payload). Either way it is the second argument the
    /// driven entrypoint received, so the record round-trips what was run.
    fn record_input(&self) -> serde_json::Value {
        match self {
            Drive::Main { input } => (*input).clone(),
            Drive::Handler { payload, .. } => (*payload).clone(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_run(
    program: &Program,
    policy: PolicyEngine,
    limits: forge_domain::Limits,
    input: &serde_json::Value,
    seed: u64,
    time_start: u64,
    recorder: RunRecorder,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    finish_drive(
        program,
        policy,
        limits,
        Drive::Main { input },
        seed,
        time_start,
        recorder,
        bridge,
    )
}

/// Drive a UI event dispatch (record or replay) and assemble its [`RunRecord`].
/// The engine runs the handler named by `action_ref` over the SAME containment /
/// limits / host path as a normal run, the handler's `ctx.*` effects are recorded
/// as usual, and the `(action_ref, payload) -> result` dispatch envelope is
/// recorded as a `ui.dispatch_event` call so the event replays byte-identically.
#[allow(clippy::too_many_arguments)]
fn finish_dispatch(
    program: &Program,
    policy: PolicyEngine,
    limits: forge_domain::Limits,
    action_ref: &str,
    payload: &serde_json::Value,
    seed: u64,
    time_start: u64,
    recorder: RunRecorder,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    finish_drive(
        program,
        policy,
        limits,
        Drive::Handler { action_ref, payload },
        seed,
        time_start,
        recorder,
        bridge,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_drive(
    program: &Program,
    policy: PolicyEngine,
    limits: forge_domain::Limits,
    drive: Drive<'_>,
    seed: u64,
    time_start: u64,
    recorder: RunRecorder,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    // The record's `input` field round-trips whatever the driven entrypoint
    // received (run input / event payload); capture it before `drive` is consumed.
    let record_input = drive.record_input();
    let mut host = HostContext::with_policy(policy, limits.clone(), recorder, bridge);
    let engine = QuickJsEngine::new();
    let outcome = match drive {
        Drive::Main { input } => engine.run(program, input, &mut host, &limits),
        Drive::Handler { action_ref, payload } => {
            let outcome = engine.run_handler(program, action_ref, payload, &mut host, &limits);
            // Record the dispatch envelope AFTER the handler's effects so the
            // trace order is: <handler's ctx.* calls> then `ui.dispatch_event`.
            // On replay the recorder consumes them in the same order, so the
            // event sequence (and thus `replay_fingerprint`) is byte-identical.
            // The recorded result is the handler's returned `value` on success
            // (the final UI tree the applet returned), or `null` on failure — a
            // determinism error here (a replay whose action_ref/payload diverge)
            // takes precedence over the handler's own outcome.
            let result = match &outcome.result {
                Ok(app) => app.value.clone(),
                Err(_) => serde_json::Value::Null,
            };
            if let Err(divergence) = host.dispatch_event(action_ref, payload.clone(), result) {
                EngineOutcome { result: Err(divergence), logs: outcome.logs }
            } else {
                outcome
            }
        }
    };

    // Capture the evaluated permission snapshot (CR-9) before consuming the host.
    let permissions = host.permission_snapshot();
    // Replay strictness (review 009 P2): a replay that ended without consuming
    // every recorded call diverged, even if no individual call mismatched. A
    // successful run that left calls behind becomes a determinism RuntimeError;
    // a run that already failed keeps its original (earlier) error.
    let consumed = host.assert_replay_consumed();
    let (recorder, logs) = host.finish();
    let calls = recorder.into_calls();

    let domain_outcome = match (outcome.result, consumed) {
        (Ok(result), Ok(())) => RunOutcome::Completed { result },
        // The run itself failed: that error is the more specific/earlier cause.
        (Err(error), _) => RunOutcome::Failed { error },
        // The run "succeeded" in JS but left recorded calls unconsumed → divergence.
        (Ok(_), Err(divergence)) => RunOutcome::Failed { error: divergence },
    };

    Ok(RunRecord::new(
        derive_run_id(program, seed, time_start),
        program.applet_id.clone(),
        program.code_hash(),
        record_input,
        seed,
        time_start,
        calls,
        logs,
        domain_outcome,
    )?
    .with_permissions(permissions))
}

/// True if any recorded call captured a policy **denial** — the exact
/// `{"denied": <CoreError>}` response written by
/// [`RunRecorder::record_denial`](crate::RunRecorder) (recorder.rs).
///
/// Review 029 P2 uses this to keep snapshotless records that contain a recorded
/// denial on the recorded (all-deny default) snapshot path, so a stripped
/// post-CR-9 denial cannot be re-granted the capability it lacked by the legacy
/// manifest fallback.
///
/// Review 035 P2: the presence of a `"denied"` *key* is NOT a unique denial
/// marker. `ctx.storage.get`/`ctx.db.get`/`ctx.db.list` replay arbitrary user
/// JSON, so a legitimate legacy run that read a stored value like
/// `{ "denied": false }` and then failed for an app reason would have been
/// mis-routed onto the all-deny path and replayed as a permission failure. The
/// denial encoding is a *specific* shape: `record_denial` writes an object with
/// exactly one key, `"denied"`, whose value is a serialized [`CoreError`]
/// (`{"kind": "...", "detail": "..."}` per its `#[serde(tag, content)]`). So we
/// match that shape exactly — a single `"denied"` key whose value deserializes as
/// a `CoreError` — instead of any object that merely carries a `"denied"` key.
/// Arbitrary user data cannot collide: a bool/string/number fails the object
/// check, and an object that lacks a valid `kind`/`detail` `CoreError` body fails
/// to deserialize.
fn trace_has_denial(calls: &[forge_domain::RecordedCall]) -> bool {
    calls.iter().any(|call| is_recorded_denial(&call.response))
}

/// True iff `response` is exactly the `{"denied": <CoreError>}` shape emitted by
/// [`RunRecorder::record_denial`](crate::RunRecorder): an object with a single
/// `"denied"` key whose value deserializes as a [`CoreError`]. See
/// [`trace_has_denial`] for why the key alone is insufficient (review 035 P2).
fn is_recorded_denial(response: &serde_json::Value) -> bool {
    let Some(obj) = response.as_object() else {
        return false;
    };
    // Exactly one key, and it is `denied`: a real denial response is `{"denied": …}`
    // and nothing else, so a stored user object that happens to include a `denied`
    // field alongside other keys is not mistaken for a denial.
    if obj.len() != 1 {
        return false;
    }
    let Some(denied) = obj.get("denied") else {
        return false;
    };
    // The value must be a serialized CoreError (`{"kind": "...", "detail": "..."}`).
    serde_json::from_value::<CoreError>(denied.clone()).is_ok()
}

/// A deterministic-but-readable run id from the program + seeds. Replays derive
/// the same id from the same inputs but carry the original `run_id` semantics at
/// the call site; tests rely on `replays_identically` (which excludes `run_id`),
/// so the id only needs to be stable and inspectable.
///
/// Review 012 P2: the displayed digest prefix is taken *after* stripping the
/// `alg:` algorithm tag, so the id is built from the digest body and is
/// algorithm-agnostic — under `sha256:` the id reads from the hash, not the
/// literal `"sha256:"` prefix the old `trim_start_matches("fnv1a64:")` left in
/// place once the algorithm changed.
fn derive_run_id(program: &Program, seed: u64, time_start: u64) -> RunId {
    let hash = program.code_hash();
    // Strip the leading `alg:` tag (everything up to and including the first
    // colon), then take a short prefix of the digest body for readability.
    let digest = hash.split_once(':').map(|(_, body)| body).unwrap_or(&hash);
    let short = &digest[..8.min(digest.len())];
    RunId::new(format!("run_{short}_{seed:x}_{time_start:x}"))
}

/// Convenience for callers that just want the `AppResult` of a fresh record run
/// (the spine's "run once" path), discarding the full record.
pub fn run_once(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    input: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<AppResult> {
    let record = record_run(program, manifest, actor, input, seed, time_start, bridge)?;
    match record.outcome {
        RunOutcome::Completed { result } => Ok(result),
        RunOutcome::Failed { error } => Err(error),
    }
}
