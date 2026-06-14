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
use forge_policy::{AllowAll, DecisionContext, PolicyEngine};

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
    // The permissive M0a default: the three trusted-source SC-10 gates allow.
    // A caller that holds real workspace/run/platform state installs it via
    // [`record_run_with_context`] so a real workspace-policy/run-profile/platform
    // deny actually blocks this live run (T037 FIX ROUND 2).
    record_run_with_context(
        program,
        manifest,
        actor,
        Box::new(AllowAll),
        input,
        seed,
        time_start,
        bridge,
    )
}

/// Like [`record_run`], but with an explicit [`DecisionContext`] — the
/// workspace-policy / run-profile / platform-permission SC-10 gates
/// ([`forge_policy::ComposedDecisionContext`]) read from **trusted** workspace /
/// run / platform state (never the request payload, review 048/050).
///
/// This is the **live** decision path forge-core's `runtime.run` wires when the
/// workspace has provisioned a real run policy: the gates are evaluated on every
/// `ctx.*` host call this run makes, so a real workspace/run/platform deny blocks
/// the live command (T037 FIX ROUND 2). The gate *outcomes* are recorded, so
/// replay (which re-installs [`AllowAll`] via [`PolicyEngine::from_snapshot`])
/// stays byte-identical without re-consulting the live sources.
#[allow(clippy::too_many_arguments)]
pub fn record_run_with_context(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    context: Box<dyn DecisionContext>,
    input: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    let recorder = RunRecorder::recording(seed, time_start);
    // `PolicyEngine::with_context` validates the manifest's storage glob grants
    // (forge-policy review 006 P2); a bare `*`/malformed grant fails closed here
    // as a ValidationError rather than being silently accepted.
    let policy = PolicyEngine::with_context(manifest, actor, context)?;
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
    // Permissive M0a default (see [`record_run`]); the trusted-context variant is
    // [`record_dispatch_with_context`].
    record_dispatch_with_context(
        program,
        manifest,
        actor,
        Box::new(AllowAll),
        action_ref,
        payload,
        seed,
        time_start,
        bridge,
    )
}

/// Like [`record_dispatch`], but with an explicit [`DecisionContext`] — the live
/// SC-10 workspace-policy / run-profile / platform-permission gates for a UI event
/// dispatch (T037 FIX ROUND 2). A UI event re-enters the handler over the SAME
/// gated host path as a run, so a real workspace/run/platform deny blocks the
/// handler's `ctx.*` calls exactly as it would in [`record_run_with_context`].
#[allow(clippy::too_many_arguments)]
pub fn record_dispatch_with_context(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    context: Box<dyn DecisionContext>,
    action_ref: &str,
    payload: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    let recorder = RunRecorder::recording(seed, time_start);
    let policy = PolicyEngine::with_context(manifest, actor, context)?;
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

/// Deliver a live-query notification to the applet's watch callback named
/// `action_ref` in **record mode**, producing a [`RunRecord`] (DL-16,
/// `forge/spec/live-queries.md` §Replay).
///
/// This is the record-side of notification DELIVERY: a committed mutation made a
/// watched record dirty, the facade computed the canonical `db.watch.notification`
/// payload, and this re-enters the callback the applet wired its `ctx.db.watch`
/// to — over the SAME containment / limits / host path as [`record_dispatch`] — to
/// let it re-render. The callback's `ctx.*` effects are recorded as usual, plus a
/// `db.watch.notification` envelope capturing the delivered payload, so the
/// notification stream replays byte-identically via [`replay_notification`].
///
/// NON-REENTRANT (T047 (a)): the callback runs in its own one-shot realm; any
/// mutation it makes through `ctx.db` is a committed write that the FACADE queues as
/// the NEXT event-loop turn (a later version), never a recursive flush inside this
/// delivery. This function delivers exactly ONE notification — the facade drives the
/// turn loop.
#[allow(clippy::too_many_arguments)]
pub fn record_notification(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    action_ref: &str,
    notification: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    // Permissive M0a default (see [`record_run`]); the trusted-context variant is
    // [`record_notification_with_context`].
    record_notification_with_context(
        program,
        manifest,
        actor,
        Box::new(AllowAll),
        action_ref,
        notification,
        seed,
        time_start,
        bridge,
    )
}

/// Like [`record_notification`], but with an explicit [`DecisionContext`] — the
/// live SC-10 workspace-policy / run-profile / platform-permission gates for a
/// live-query notification delivery (T037 FIX ROUND 2). The watch callback runs
/// over the SAME gated host path as a dispatch, so a real workspace/run/platform
/// deny blocks its `ctx.*` calls.
#[allow(clippy::too_many_arguments)]
pub fn record_notification_with_context(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    context: Box<dyn DecisionContext>,
    action_ref: &str,
    notification: &serde_json::Value,
    seed: u64,
    time_start: u64,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    let recorder = RunRecorder::recording(seed, time_start);
    let policy = PolicyEngine::with_context(manifest, actor, context)?;
    finish_drive(
        program,
        policy,
        manifest.limits.clone(),
        Drive::Notification { action_ref, notification },
        seed,
        time_start,
        recorder,
        bridge,
    )
}

/// Replay a recorded live-query notification delivery (the counterpart to
/// [`record_notification`]). Re-runs the same callback in **replay mode**: the
/// recorder serves the recorded `ctx.*` responses and asserts the delivered
/// notification payload matches the recording, so the produced record must
/// [`replays_identically`](RunRecord::replays_identically) to `run`. A diverging
/// notification (different payload, or order) is a determinism `RuntimeError`. The
/// recorded permission snapshot governs the replay exactly as in [`replay`] (CR-9).
///
/// `action_ref` is the watch's registered callback handler — workspace state the
/// FACADE holds (the notification payload does not name its callback), so the caller
/// supplies it. The delivered notification payload itself is recovered from the
/// record's `db.watch.notification` envelope so the replay re-issues the SAME
/// delivery.
pub fn replay_notification(
    run: &RunRecord,
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    action_ref: &str,
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
    let notification = recorded_notification(run)?;
    let recorder = RunRecorder::replaying(run.random_seed, run.time_start, run.calls.clone());
    let (policy, host_call_cap) = replay_policy(run, manifest, actor)?;
    let mut limits = manifest.limits.clone();
    limits.max_host_calls = host_call_cap;
    finish_drive(
        program,
        policy,
        limits,
        Drive::Notification { action_ref, notification: &notification },
        run.random_seed,
        run.time_start,
        recorder,
        bridge,
    )
}

/// Recover the delivered `db.watch.notification` payload from a recorded
/// notification-delivery run (its `args` is the canonical payload). A record built
/// by [`record_notification`] carries exactly one such call; its absence is a
/// `RuntimeError` (the record is not a notification record).
fn recorded_notification(run: &RunRecord) -> Result<serde_json::Value> {
    run.calls
        .iter()
        .find(|c| c.method == "db.watch.notification")
        .map(|c| c.args.clone())
        .ok_or_else(|| {
            CoreError::RuntimeError(
                "replay_notification: record carries no db.watch.notification envelope".into(),
            )
        })
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
    /// Deliver a live-query notification: re-enter the watch callback
    /// `<action_ref>(ctx, notification)` and record the `db.watch.notification`
    /// envelope so the notification stream replays byte-identically (DL-16,
    /// `forge/spec/live-queries.md` §Replay). This reuses the SAME containment /
    /// limits / host path as a UI dispatch (the callback is an exported handler);
    /// it differs only in which envelope is recorded around the callback's effects.
    Notification {
        action_ref: &'a str,
        notification: &'a serde_json::Value,
    },
}

impl<'a> Drive<'a> {
    /// The value recorded as the run record's `input` (the `main` input, the
    /// dispatched event's payload, or the delivered notification). Either way it is
    /// the second argument the driven entrypoint received, so the record round-trips
    /// what was run.
    fn record_input(&self) -> serde_json::Value {
        match self {
            Drive::Main { input } => (*input).clone(),
            Drive::Handler { payload, .. } => (*payload).clone(),
            Drive::Notification { notification, .. } => (*notification).clone(),
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
        Drive::Notification { action_ref, notification } => {
            // Re-enter the watch callback over the SAME engine path as a UI
            // dispatch: the notification IS the callback's event payload
            // (`<callback>(ctx, notification)`). Record the `db.watch.notification`
            // envelope AFTER the callback's effects so the trace order is
            // <callback's ctx.* calls> then the notification envelope, and on replay
            // the recorder consumes them in the same order — the notification stream
            // (and thus `replay_fingerprint`) is byte-identical (DL-16 §Replay).
            let outcome = engine.run_handler(program, action_ref, notification, &mut host, &limits);
            if let Err(divergence) = host.deliver_notification(notification.clone()) {
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
/// denial encoding is a *specific* shape: a `"denied"` key whose value is a
/// serialized [`CoreError`] (`{"kind": "...", "detail": "..."}` per its
/// `#[serde(tag, content)]`), optionally accompanied by the non-sensitive boolean
/// `"secret_injected"` marker (review 153). So we match that shape exactly instead
/// of any object that merely carries a `"denied"` key. Arbitrary user data cannot
/// collide: a bool/string/number fails the object check, an object that lacks a
/// valid `kind`/`detail` `CoreError` body fails to deserialize, and any extra key
/// other than the allowed `secret_injected` boolean disqualifies it.
fn trace_has_denial(calls: &[forge_domain::RecordedCall]) -> bool {
    calls.iter().any(|call| is_recorded_denial(&call.response))
}

/// True iff `response` is the denial shape emitted by the recorder: an object
/// carrying a `"denied"` key whose value deserializes as a [`CoreError`], with at
/// most an optional boolean `"secret_injected"` marker alongside it.
///
/// Two recorder paths write this shape (review 153 / review 155):
/// * [`RunRecorder::record_denial`](crate::RunRecorder) writes the single-key
///   `{"denied": <CoreError>}` for a request-gate denial (nothing was sent).
/// * [`RunRecorder::redact_last_response`](crate::RunRecorder) writes
///   `{"denied": <CoreError>, "secret_injected": true}` for a response-leg denial
///   that already injected a secret over the wire, so the audit builder can still
///   emit the `secret.use` row.
///
/// Both must be recognized as a recorded denial, so a snapshotless record whose
/// only denial is a response-leg-after-injection net deny stays on the recorded
/// (all-deny default) snapshot path instead of falling back to the live permissive
/// manifest (the tampered-replay attack at the top of [`replay_policy`]'s doc).
/// This mirrors the parallel recognizer in [`crate::host::net`] (`denied` present
/// AND `status` absent). See [`trace_has_denial`] for why the key alone is
/// insufficient (review 035 P2) — arbitrary multi-key user JSON is still rejected.
fn is_recorded_denial(response: &serde_json::Value) -> bool {
    let Some(obj) = response.as_object() else {
        return false;
    };
    // The denial-bearing key must be present and a serialized CoreError
    // (`{"kind": "...", "detail": "..."}`).
    let Some(denied) = obj.get("denied") else {
        return false;
    };
    if serde_json::from_value::<CoreError>(denied.clone()).is_err() {
        return false;
    }
    // Only `denied`, and (optionally) a boolean `secret_injected`, may appear —
    // any other key means this is arbitrary stored user JSON, not a recorded
    // denial (review 035 P2 collision guard). The single-key `{"denied": …}`
    // request-gate shape and the two-key `{"denied": …, "secret_injected": true}`
    // response-leg shape (review 153) both pass; anything else is rejected.
    obj.iter().all(|(key, value)| match key.as_str() {
        "denied" => true,
        "secret_injected" => value.is_boolean(),
        _ => false,
    })
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

#[cfg(test)]
mod denial_guard_tests {
    use super::*;
    use crate::recorder::RunRecorder;
    use forge_domain::{
        Capabilities, Limits, NetGrant, NetRule, RecordedCall, StorageGrant,
    };

    /// Build the exact response-leg-after-injection denial shape via the REAL
    /// recorder producer (`redact_last_response` with `secret_injected = true`),
    /// not a hand-written literal — so this test tracks the producer (review 153).
    fn response_leg_injected_denial() -> serde_json::Value {
        let mut rec = RunRecorder::recording(1, 0);
        // First a produced net.fetch call (its response is captured), then the
        // response-leg policy denies AFTER the secret was already injected/sent.
        rec.host_call("net.fetch", serde_json::json!({"url": "x"}), || {
            Ok(serde_json::json!({"status": 200, "body": "REJECTED"}))
        })
        .unwrap();
        rec.redact_last_response(
            &CoreError::PermissionDenied("redirect to private host".into()),
            true,
        );
        let calls = rec.into_calls();
        calls.last().unwrap().response.clone()
    }

    /// Review 155 (P1 security): the two-key `{"denied": <CoreError>,
    /// "secret_injected": true}` response-leg denial shape MUST be recognized as a
    /// recorded denial — otherwise a snapshotless record whose only denial is a
    /// response-leg-after-injection net deny falls through to the permissive
    /// manifest fallback (the tampered-replay attack).
    #[test]
    fn response_leg_injected_denial_is_recognized() {
        let response = response_leg_injected_denial();
        assert_eq!(
            response.get("secret_injected").and_then(|v| v.as_bool()),
            Some(true),
            "precondition: producer emits the two-key shape: {response}"
        );
        assert!(
            is_recorded_denial(&response),
            "the {{denied, secret_injected}} shape must be recognized as a denial: {response}"
        );
        // And the single-key request-gate shape is still recognized.
        let single = serde_json::json!({
            "denied": CoreError::PermissionDenied("nope".into())
        });
        assert!(is_recorded_denial(&single), "the single-key denial shape stays recognized");
    }

    /// The collision guard (review 035 P2) still holds: arbitrary stored user JSON
    /// — including a `secret_injected` value that is NOT a boolean, or an extra
    /// unrelated key — is NOT a recorded denial.
    #[test]
    fn arbitrary_user_json_is_not_a_denial() {
        // A user object that merely carries a `denied` field alongside others.
        assert!(!is_recorded_denial(&serde_json::json!({ "denied": false })));
        assert!(!is_recorded_denial(&serde_json::json!({ "denied": false, "x": 1 })));
        // A valid CoreError `denied` plus an UNEXPECTED extra key is rejected.
        let err = CoreError::PermissionDenied("x".into());
        assert!(!is_recorded_denial(&serde_json::json!({ "denied": err, "evil": 1 })));
        // A valid CoreError `denied` plus a NON-boolean `secret_injected` is rejected.
        let err = CoreError::PermissionDenied("x".into());
        assert!(!is_recorded_denial(
            &serde_json::json!({ "denied": err, "secret_injected": "yes" })
        ));
        // Non-objects are never denials.
        assert!(!is_recorded_denial(&serde_json::json!("denied")));
        assert!(!is_recorded_denial(&serde_json::json!(null)));
    }

    /// Review 155 (P1 security): a snapshotless record (loads as the all-deny
    /// default) whose ONLY denial is the response-leg-after-injection net deny must
    /// stay on the recorded all-deny snapshot path — NOT fall back to the live
    /// permissive manifest. Proven by the host-call cap selected: the snapshot path
    /// uses `run.permissions.max_host_calls` (0 for the all-deny default), the
    /// manifest fallback uses `manifest.limits.max_host_calls`. If the two-key
    /// denial were unrecognized, `use_manifest_fallback` would be true and the cap
    /// would be the (permissive) manifest's — re-granting the denied capability.
    #[test]
    fn stripped_response_leg_injected_denial_stays_on_all_deny_snapshot() {
        // A snapshotless record: default (all-deny) permissions, and its only call
        // is the response-leg-after-injection denial.
        let run = RunRecord {
            run_id: RunId::new("run_x"),
            applet_id: forge_domain::AppletId::new("app_test"),
            code_hash: "sha256:00".into(),
            input: serde_json::Value::Null,
            random_seed: 1,
            time_start: 0,
            calls: vec![RecordedCall {
                seq: 0,
                method: "net.fetch".into(),
                args: serde_json::json!({"url": "x"}),
                response: response_leg_injected_denial(),
            }],
            logs: vec![],
            outcome: RunOutcome::Failed {
                error: CoreError::PermissionDenied("redirect to private host".into()),
            },
            permissions: PermissionSnapshot::default(),
        };
        assert_eq!(run.permissions, PermissionSnapshot::default());
        assert!(trace_has_denial(&run.calls), "the recorded call is a denial");

        // A PERMISSIVE manifest with a non-zero host-call cap and a granting actor:
        // if the fallback engaged, the replay policy would carry THIS cap and grant.
        let manifest = Manifest {
            entrypoint: "main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: Capabilities {
                storage: StorageGrant {
                    read: vec!["app/*".into()],
                    write: vec!["app/*".into()],
                },
                net: NetGrant(vec![NetRule {
                    method: "GET".into(),
                    url: "https://api.example.com/private/*".into(),
                    ..Default::default()
                }]),
                ..Capabilities::default()
            },
            limits: Limits { max_host_calls: 99, ..Limits::default() },
        };
        let actor = ActorContext::owner("dev");

        let (_policy, cap) = replay_policy(&run, &manifest, &actor).unwrap();
        // The all-deny snapshot path is selected: the cap is the snapshot's 0, NOT
        // the permissive manifest's 99. A regression (unrecognized two-key denial)
        // would pick 99 and re-grant the denied capability.
        assert_eq!(
            cap, 0,
            "a recorded response-leg-injected denial must keep replay on the all-deny snapshot, not the permissive manifest"
        );
    }
}
