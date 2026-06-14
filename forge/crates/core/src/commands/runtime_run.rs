//! `runtime.run` — record one deterministic run of an installed applet (CR-A2,
//! CR-8, CR-9). Moved verbatim from `workspace.rs` (/simplify #11a): the handler
//! plus its response-shaping helpers ([`outcome_fields`] / [`run_summary`]).

use forge_domain::{CoreError, Result, RunRecord};
use forge_runtime::{record_run, Program as RuntimeProgram};

use crate::determinism::{derive_seeds, run_seed_override, unique_run_id};
use crate::StorageHostBridge;

use super::super::{AppletLifecycle, WorkspaceCore};
use super::lifecycle::{not_installed, LIFECYCLE_NOT_INSTALLED, LIFECYCLE_SUSPENDED};
use super::require_applet_id;

impl WorkspaceCore {
    /// `runtime.run` — load the compiled program, run it via the QuickJS engine
    /// in record mode with a [`StorageHostBridge`] + the applet's policy
    /// (manifest capabilities), save the [`RunRecord`], emit
    /// `run.started`/`ui.patch`/`run.completed`, and return the run summary +
    /// `AppResult` (CR-A2, CR-8, CR-9).
    ///
    /// Payload: `{ applet_id, input, random_seed?, time_start? }`.
    ///
    /// `random_seed`/`time_start` are **optional** deterministic-seam overrides
    /// (review 032 finding 1). When present they pin the run's RNG/clock seeds to
    /// exact values — the conformance corpus uses this to drive a scenario
    /// recorded under specific seeds (e.g. `seeded_random` under seed 7) through
    /// this facade rather than a parallel direct path. When absent the seeds are a
    /// deterministic function of `(code_hash, input)` (the default that makes
    /// independent re-runs of the demo replay identically; review 031 finding 2).
    pub(in crate::workspace) fn cmd_runtime_run(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let input = cmd.payload.get("input").cloned().unwrap_or(serde_json::Value::Null);
        let seed_override = run_seed_override(cmd)?;

        // Lifecycle gate (CR-7 / `forge/spec/applet-lifecycle.md`): a run is allowed
        // ONLY for an `enabled` applet. Reject an UNINSTALLED applet (no active
        // record) and a SUSPENDED applet BEFORE any user code starts — no host calls,
        // no UI patches, no records touched (the `run_uninstalled_rejected` /
        // suspend vectors). Each rejection emits a `runtime.run.rejected` audit event
        // carrying the stable lifecycle marker so the pre-run denial is observable.
        let installed = match self.load_applet(applet_id.as_str())? {
            Some(installed) => installed,
            None => {
                let error = not_installed(applet_id.as_str());
                self.emit_run_rejected(&applet_id, LIFECYCLE_NOT_INSTALLED, &error);
                return Err(error);
            }
        };
        if self.applet_lifecycle(applet_id.as_str())? == AppletLifecycle::Suspended {
            let error = CoreError::ValidationError(format!(
                "{LIFECYCLE_SUSPENDED}: applet {applet_id} is suspended; runtime.run is rejected before user code starts"
            ));
            self.emit_run_rejected(&applet_id, LIFECYCLE_SUSPENDED, &error);
            return Err(error);
        }

        self.events.emit(
            Some(applet_id.clone()),
            "run.started",
            serde_json::json!({ "applet_id": applet_id, "code_hash": installed.code_hash }),
        );

        let program = RuntimeProgram::new(applet_id.clone(), installed.js_code.clone());
        // Sanity: the runtime hashes the SAME js bytes the pipeline did, so its
        // recorded code_hash equals the pipeline's. Asserted in the integration
        // test; here we surface a clear error if that invariant ever breaks.
        if program.code_hash() != installed.code_hash {
            return Err(CoreError::RuntimeError(format!(
                "code_hash provenance broken: runtime {} != pipeline {}",
                program.code_hash(),
                installed.code_hash
            )));
        }

        // Replay seeds are a deterministic function of (code_hash, input): a
        // re-run with the SAME applet code and input reproduces the SAME seeded
        // time/random seams, so the two records replay byte-identically. A
        // different input yields different seeds (a genuinely different run).
        // An explicit `(random_seed, time_start)` in the payload overrides this
        // default so a conformance scenario recorded under fixed seeds can be
        // reproduced through the facade (review 032 finding 1). Either way the
        // chosen seeds are recorded on the run, so replay stays byte-identical.
        let (random_seed, time_start) =
            seed_override.unwrap_or_else(|| derive_seeds(&installed.code_hash, &input));

        // Mint a UNIQUE per-execution run id (review 031 finding 2 / CR-9). The
        // counter is persisted in workspace meta so each execution is saved and
        // loadable separately, even when code+input (and therefore the seeds)
        // match a prior run. This bump happens before the run so the id is
        // reserved even if the run itself fails.
        let invocation = self.next_run_counter()?;

        // Build this run's `ctx.net.fetch` client from the injected factory BEFORE
        // borrowing the store mutably for the bridge (the factory closure borrows
        // `&self`; the bridge borrows `&mut self.store`). Default = NoNetworkClient
        // (fail-closed, no live network) unless a host/shell injected a real client
        // via `set_http_client_factory`.
        let http_client = (self.http_client_factory)();
        // Build this run's secret store from the injected factory too (SC-13): the
        // host resolves a `secret_ref` header's value against it at the HTTP edge,
        // inside the runtime's recording closure. Default = empty (fail-closed).
        let secret_store = (self.secret_store_factory)();
        // Build this run's `ctx.files` sandbox filesystem from the injected factory
        // (CR-3 / spec/files.md). It carries the TRUSTED handle → per-applet-root
        // resolution; the runtime resolves a granted handle to its sandbox root and
        // performs a capability-checked, confined read/write at the host edge.
        // Default = empty (no granted root → every ctx.files op fails closed).
        let file_system = (self.file_system_factory)();

        // Run in record mode against the live Store-backed bridge. The bridge
        // performs the SQLite writes / UI diffs; the runtime's HostContext gates
        // each ctx.* call against the manifest policy BEFORE the bridge runs —
        // including the SC-5 net egress check (so a denied ctx.net.fetch never
        // reaches the injected client) and the CR-3 files capability + sandbox
        // confinement (so a denied/escaping ctx.files op never reaches the
        // injected filesystem). The files grant the runtime gates against is read
        // from the TRUSTED manifest snapshot, not the request payload.
        let mut bridge = StorageHostBridge::with_http_client(
            &mut self.store,
            applet_id.as_str(),
            http_client,
        )
        .with_secret_store(secret_store)
        .with_file_system(file_system);
        let mut run = record_run(
            &program,
            &installed.manifest,
            &cmd.actor,
            &input,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // Drain the captured UI renders + logs before dropping the bridge (which
        // releases the &mut Store borrow so we can save the run).
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        drop(bridge);

        // Override the runtime's deterministic run_id with the unique per-core
        // identity so two executions of the same applet+input persist as two
        // distinct, independently loadable records (the seeds/trace are
        // unchanged, so each still replays identically to itself).
        run.run_id = unique_run_id(&run.code_hash, invocation);

        // Pin the replay artifact PER RUN: persist the EXACT compiled program +
        // manifest this run executed, keyed by `run_id`, so replay reconstructs
        // the program AND the manifest (engine limits/capabilities) from what this
        // run actually used — never from whatever is installed now (review 031
        // finding 3 / review 036 finding 2; CR-9 version-pinned replay).
        //
        // Review 036 finding 2: the prior pin was keyed by `code_hash` alone, so
        // reinstalling the SAME JS under a different manifest (tighter `limits`,
        // changed legacy caps) overwrote `program/<code_hash>` and stranded older
        // runs' context — replay then used the new manifest's engine limits. The
        // per-run key is unique to this execution, so no reinstall (same code or
        // not) can overwrite it. The content-addressed `program/<code_hash>` pin
        // is kept as a fallback for legacy runs recorded before per-run pinning —
        // and is now WRITE-ONCE (review 038 finding 3), so even a same-JS reinstall
        // under a different manifest can no longer overwrite the fallback a legacy
        // run depends on.
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;

        // Persist the deterministic run record (replay source, CR-9). save_run
        // re-validates the canonical code_hash.
        self.store.save_run(&run)?;

        // Emit a ui.patch event per render (the UI tree patch link).
        for (i, render) in ui_renders.iter().enumerate() {
            self.events.emit(
                Some(applet_id.clone()),
                "ui.patch",
                serde_json::json!({
                    "applet_id": applet_id,
                    "render_index": i,
                    "tree": render.tree,
                    "patches": render.patches,
                }),
            );
        }

        let summary = run_summary(&run);
        let (ok, app_result) = outcome_fields(&run);

        // Persist the run's LAST rendered tree as the applet's last-known tree — the
        // diff base for the next UI event (UI-4/CR-6). `runtime.run`'s `main` is the
        // initial render of the interactive session; a subsequent `ui.dispatch_event`
        // diffs its handler's new tree against this one. Only on a completed run with
        // at least one render (a failed/no-render run leaves the prior base intact).
        if ok {
            if let Some(last) = ui_renders.last() {
                self.store_ui_tree(applet_id.as_str(), &last.tree)?;
            }
        }
        let event_kind = if ok { "run.completed" } else { "run.failed" };
        self.events.emit(
            Some(applet_id.clone()),
            event_kind,
            serde_json::json!({ "run_id": run.run_id, "ok": ok }),
        );

        Ok(serde_json::json!({
            "run_id": run.run_id,
            "code_hash": run.code_hash,
            "ok": ok,
            "result": app_result,
            "summary": summary,
            // The ordered host-call method trace the run issued (`db.insert`,
            // `ui.render`, …). Surfaced so a shell / conformance harness can
            // assert the exact effect sequence through the facade rather than
            // re-reading the persisted RunRecord (review 032 finding 1).
            "host_call_methods": run.calls.iter().map(|c| c.method.clone()).collect::<Vec<_>>(),
            "ui_renders": ui_renders.iter().map(|r| r.tree.clone()).collect::<Vec<_>>(),
        }))
    }

    /// Emit a `runtime.run.rejected` audit event for a run denied by the lifecycle
    /// gate BEFORE any user code ran (CR-7). Carries the stable lifecycle
    /// `error_code` marker plus the typed error message so a host/auditor can react
    /// to the pre-run denial without parsing English text — the run path's analogue
    /// of `ui.dispatch_event`'s `ui.dispatch_event.rejected`.
    fn emit_run_rejected(
        &mut self,
        applet_id: &forge_domain::AppletId,
        error_code: &str,
        error: &CoreError,
    ) {
        self.events.emit(
            Some(applet_id.clone()),
            "runtime.run.rejected",
            serde_json::json!({
                "applet_id": applet_id,
                "error_code": error_code,
                "message": error.to_string(),
            }),
        );
    }
}

/// `(ok, app_result_json)` for a run's outcome.
pub(in crate::workspace) fn outcome_fields(run: &RunRecord) -> (bool, serde_json::Value) {
    use forge_domain::RunOutcome;
    match &run.outcome {
        RunOutcome::Completed { result } => {
            (result.ok, serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
        }
        RunOutcome::Failed { error } => (false, serde_json::json!({ "error": error })),
    }
}

/// A compact summary of a run for the response payload + observability.
pub(in crate::workspace) fn run_summary(run: &RunRecord) -> serde_json::Value {
    serde_json::json!({
        "run_id": run.run_id,
        "applet_id": run.applet_id,
        "code_hash": run.code_hash,
        "calls": run.calls.len(),
        "logs": run.logs.len(),
        "completed": run.is_completed(),
    })
}
