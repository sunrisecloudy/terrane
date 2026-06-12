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
use crate::{JsEngine, Program};
use forge_domain::{
    ActorContext, AppResult, CoreError, Manifest, Result, RunId, RunOutcome, RunRecord,
};

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
    finish_run(
        program, manifest, actor, input, seed, time_start, recorder, bridge,
    )
}

/// Replay a previously recorded [`RunRecord`] by re-running `program` in
/// **replay mode**. The recorder serves the recorded responses and detects any
/// divergence (→ `RuntimeError`). `bridge` should be a
/// [`NullBridge`](crate::NullBridge) (or any bridge): replay never touches live
/// reads/seams, so the bridge is only a safety net.
///
/// The produced record must `replays_identically` to `run`; callers/tests
/// assert this to prove determinism.
pub fn replay(
    run: &RunRecord,
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    manifest.validate()?;
    // Guard: replaying different code than was recorded is a determinism error.
    if program.code_hash() != run.code_hash {
        return Err(CoreError::RuntimeError(format!(
            "determinism divergence: replay program code_hash {} != recorded {}",
            program.code_hash(),
            run.code_hash
        )));
    }
    let recorder = RunRecorder::replaying(run.random_seed, run.time_start, run.calls.clone());
    finish_run(
        program,
        manifest,
        actor,
        &run.input,
        run.random_seed,
        run.time_start,
        recorder,
        bridge,
    )
}

/// Shared body for record/replay: drive the engine with a prepared recorder and
/// assemble the [`RunRecord`].
#[allow(clippy::too_many_arguments)]
fn finish_run(
    program: &Program,
    manifest: &Manifest,
    actor: &ActorContext,
    input: &serde_json::Value,
    seed: u64,
    time_start: u64,
    recorder: RunRecorder,
    bridge: &mut dyn HostBridge,
) -> Result<RunRecord> {
    let mut host = HostContext::new(manifest, actor, recorder, bridge);
    let engine = QuickJsEngine::new();
    let outcome = engine.run(program, input, &mut host, &manifest.limits);
    let (recorder, logs) = host.finish();
    let calls = recorder.into_calls();

    let domain_outcome = match outcome.result {
        Ok(result) => RunOutcome::Completed { result },
        Err(error) => RunOutcome::Failed { error },
    };

    Ok(RunRecord {
        run_id: derive_run_id(program, seed, time_start),
        applet_id: program.applet_id.clone(),
        code_hash: program.code_hash(),
        input: input.clone(),
        random_seed: seed,
        time_start,
        calls,
        logs,
        outcome: domain_outcome,
    })
}

/// A deterministic-but-readable run id from the program + seeds. Replays derive
/// the same id from the same inputs but carry the original `run_id` semantics at
/// the call site; tests rely on `replays_identically` (which excludes `run_id`),
/// so the id only needs to be stable and inspectable.
fn derive_run_id(program: &Program, seed: u64, time_start: u64) -> RunId {
    RunId::new(format!(
        "run_{}_{seed:x}_{time_start:x}",
        &program.code_hash().trim_start_matches("fnv1a64:")[..8.min(program.code_hash().len())]
    ))
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
