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
        // The live watch registry (rebuilt from the persisted sessions) injected into
        // the bridge so each `ctx.db` write captures the pre-write watch membership for
        // its live-query notification turn (DL-16). Built BEFORE the bridge borrows
        // `&mut self.store` (the rebuild borrows `&self`).
        let watch_registry = self.watch_sessions.to_registry()?;
        // The watch ids owned by OTHER applets, injected so a `ctx.db.watch` of a
        // foreign-owned id is rejected AT HOST-CALL TIME with `PermissionDenied` (a
        // recorded run denial), not silently accepted and dropped after the run by the
        // owner-scoped intent fold (review 135 #1).
        let foreign_watch_ids = self.watch_sessions.foreign_owned_watch_ids(applet_id.as_str());

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
        .with_file_system(file_system)
        .with_watch_registry(watch_registry)
        .with_foreign_watch_ids(foreign_watch_ids);
        let mut run = record_run(
            &program,
            &installed.manifest,
            &cmd.actor,
            &input,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // Drain the captured UI renders + watch intents before dropping the bridge
        // (which releases the &mut Store borrow so we can save the run + fold the
        // intents). A `ctx.db.watch`/`unwatch` the run issued is captured here as an
        // intent (DL-16); we register/cancel it against the workspace registry after
        // the borrow is released.
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        let watch_intents = std::mem::take(&mut bridge.watch_intents);
        // The record-mutating writes the run COMMITTED through `ctx.db` (already
        // applied to the store). We drive their live-query notifications AFTER the
        // borrow is released, so a watch fires on a real applet mutation (DL-16).
        let applied_mutations = std::mem::take(&mut bridge.applied_mutations);
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

        // SC-12 live wiring (`forge/spec/audit-log.md`): the security-relevant
        // capability USES this real run made — each `ctx.net.fetch` egress and each
        // `secret_ref` header it resolved — land durable, queryable `network.egress` /
        // `secret.use` audit rows, derived from the recorded host-call trace. The trace
        // already keeps only the `secret_ref` (never the resolved value), and the
        // persistence layer redacts request/response bodies, so no secret value or body
        // is ever written. Best-effort: an audit-persistence error never fails the run
        // (the run + its effects already committed) — we never lose the run over its
        // audit trail.
        let _ = self.persist_run_egress_audit(applet_id.as_str(), &cmd.actor, &run);

        // Fold any `ctx.db.watch`/`unwatch` the run issued into the workspace
        // live-query registry (DL-16) and persist, so a watch an applet registered
        // during its run is live for subsequent committed mutations. A failed run
        // still folds the intents it issued before failing (the run's effects up to
        // the failure already committed through the live bridge). This runs BEFORE we
        // notify the run's own writes, so a watch the run registered in `main` sees
        // a record `main` then inserted in the same run (registration precedes the
        // write's notification turn).
        self.apply_watch_intents(applet_id.as_str(), &watch_intents)?;

        // Deliver live-query notifications for the record writes the run committed
        // through `ctx.db` (DL-16): each captured write is driven as a committed-
        // transaction notification turn (its own monotone version, in apply order),
        // a watch's `onWatch` callback is re-entered, and any callback mutation is
        // queued as the next turn (non-reentrant). The writes already landed; this
        // computes their notifications without re-applying them.
        self.notify_committed_mutations(applet_id.as_str(), &applied_mutations)?;

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

    /// Persist the SC-12 `network.egress` + `secret.use` audit rows for a real run's
    /// recorded `ctx.net.fetch` host calls (`forge/spec/audit-log.md`; the
    /// `audit-log-e2e` `network_egress_metadata_no_body` / `secret_access_redacted`
    /// vectors). Walks the run's recorded host-call trace in call order.
    ///
    /// DENY classification (review 151): a fetch the policy DENIED was recorded as
    /// `{"denied": <CoreError>}` (it never reached the network and was never
    /// approved). Such a call appends a SINGLE `network.egress` `deny` row carrying
    /// the denial reason and `{method, scheme, host, path}` metadata — and emits NO
    /// `allow` egress/secret rows and NO defaulted `status: 0` — so a forbidden egress
    /// or a disallowed secret header is auditable AS a denial, never as an approval.
    ///
    /// For an ALLOWED `net.fetch` (a real `NetResponse` was served) it appends:
    ///
    ///   - one `network.egress` row — `resource_type = network`,
    ///     `resource_id = scheme://host`, metadata `{method, scheme, host, path, status,
    ///     request_body_redacted, response_body_redacted}`. The request/response
    ///     bodies are handed to the persistence layer, which REDACTS them (the
    ///     `request_body`/`response_body` keys are dropped + the `*_redacted` markers
    ///     stamped), so no body is ever stored;
    ///   - one `secret.use` row PER `secret_ref` header the request carried —
    ///     `resource_type = secret`, `resource_id = <secret_ref>`, metadata
    ///     `{secret_ref, target_host, target_header, value_redacted}`. The recorded
    ///     trace already keeps only the `secret_ref` (the resolved value is injected at
    ///     the HTTP edge and never recorded), so no secret value can be present.
    ///
    /// Deterministic: each row's `logical_time` is the EventSink clock; the rows
    /// derive purely from the recorded trace, so a replayed run reproduces them
    /// byte-identically. Append-only; a re-run appends fresh rows.
    fn persist_run_egress_audit(
        &mut self,
        applet_id: &str,
        actor: &forge_domain::ActorContext,
        run: &RunRecord,
    ) -> Result<()> {
        let actor_id = actor.actor.as_str().to_string();
        for call in &run.calls {
            if call.method != "net.fetch" {
                continue;
            }
            let request = &call.args;
            let method = request
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = request.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let (scheme, host, path) = split_url(url);

            // DENY classification (review 151). The recorder stores a host call the
            // policy DENIED before/around the bridge as `{"denied": <CoreError>}` (a
            // non-allowlisted fetch, a forbidden secret header, a response-leg cap
            // violation — `runtime/src/recorder.rs::record_denial` /
            // `redact_last_response`). Such a fetch NEVER reached the live network and
            // was NEVER approved, so it must NOT mint `allow` `network.egress`/
            // `secret.use` rows nor default its status to `0/allow`. Instead emit a
            // SINGLE `network.egress` `deny` row carrying the denial reason (the
            // request's `secret_ref` headers are still withheld — no value is present
            // to leak, and a denied fetch resolved no secret), so a forbidden egress
            // is auditable AS a denial rather than persisted as an approval.
            if let Some(reason) = denied_reason(&call.response) {
                self.persist_producer_audit(
                    "network.egress.denied",
                    serde_json::json!({
                        "decision": "deny",
                        "applet_id": applet_id,
                        "actor_id": actor_id,
                        "method": method,
                        "host": host,
                    }),
                    "net",
                    "network.egress",
                    "deny",
                    actor_id.clone(),
                    "network",
                    Some(format!("{scheme}://{host}")),
                    None,
                    reason,
                    serde_json::json!({
                        "method": method,
                        "scheme": scheme,
                        "host": host,
                        "path": path,
                    }),
                )?;
                continue;
            }

            // The fetch was ALLOWED — it reached the bridge and returned a real
            // `NetResponse` (always carrying a numeric `status`). Only now do the
            // `allow` egress/secret producers run.
            let status = call
                .response
                .get("status")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // One secret.use row PER secret_ref header the request carried. The trace
            // holds the secret_ref name only; we pass `value_redacted` so the row
            // records that a value was withheld (no value is present to redact).
            if let Some(headers) = request.get("headers").and_then(|v| v.as_object()) {
                for (header_name, header_value) in headers {
                    let secret_ref = header_value
                        .get("secret_ref")
                        .and_then(|v| v.as_str());
                    if let Some(secret_ref) = secret_ref {
                        self.persist_producer_audit(
                            "secret.used",
                            serde_json::json!({
                                "decision": "allow",
                                "applet_id": applet_id,
                                "actor_id": actor_id,
                                "secret_ref": secret_ref,
                            }),
                            "secrets",
                            "secret.use",
                            "allow",
                            actor_id.clone(),
                            "secret",
                            Some(secret_ref.to_string()),
                            None,
                            "secret_ref injected into allowlisted header",
                            serde_json::json!({
                                "secret_ref": secret_ref,
                                "target_host": host,
                                "target_header": header_name,
                                "value_redacted": true,
                            }),
                        )?;
                    }
                }
            }

            // One network.egress row for the fetch. The request/response BODIES are
            // handed in raw and DROPPED by the persistence redaction layer (no body is
            // ever stored); method/scheme/host/path/status survive.
            let mut metadata = serde_json::Map::new();
            metadata.insert("method".into(), serde_json::json!(method));
            metadata.insert("scheme".into(), serde_json::json!(scheme));
            metadata.insert("host".into(), serde_json::json!(host));
            metadata.insert("path".into(), serde_json::json!(path));
            metadata.insert("status".into(), serde_json::json!(status));
            if let Some(body) = request.get("body") {
                metadata.insert("request_body".into(), body.clone());
            }
            if let Some(body) = call.response.get("body") {
                metadata.insert("response_body".into(), body.clone());
            }
            self.persist_producer_audit(
                "network.egress",
                serde_json::json!({
                    "decision": "allow",
                    "applet_id": applet_id,
                    "actor_id": actor_id,
                    "method": method,
                    "host": host,
                }),
                "net",
                "network.egress",
                "allow",
                actor_id.clone(),
                "network",
                Some(format!("{scheme}://{host}")),
                None,
                "network policy allowed request",
                serde_json::Value::Object(metadata),
            )?;
        }
        Ok(())
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

/// Classify a recorded `net.fetch` response as a policy DENIAL (review 151).
/// The recorder stores a host call rejected by policy as `{"denied": <CoreError>}`
/// (`runtime/src/recorder.rs::record_denial` / `redact_last_response`); a real
/// served response is a full `NetResponse` (always carrying a numeric `status`), so
/// a lone `denied` key is unambiguously the denial shape. Returns the denial reason
/// (`"<Code>: <message>"`) when the call was denied, else `None` for an allowed
/// fetch. The reason is reconstructed from the recorded `CoreError` when it decodes,
/// else degrades to a generic marker (the row still records that the fetch was
/// denied), and never carries a body — the recorder already redacted the response.
fn denied_reason(response: &serde_json::Value) -> Option<String> {
    // A lone `denied` key (an object of exactly one entry) is the redaction shape; a
    // real `NetResponse` always carries more fields (at least `status`), so it never
    // matches here.
    if response.as_object().is_none_or(|o| o.len() != 1) {
        return None;
    }
    let denied = response.get("denied")?;
    let reason = match serde_json::from_value::<CoreError>(denied.clone()) {
        Ok(err) => format!("{}: {err}", err.code()),
        Err(_) => "network egress denied by policy".to_string(),
    };
    Some(reason)
}

/// Split an absolute request URL into `(scheme, host, path)` for the SC-12
/// `network.egress` audit metadata. `https://api.example.com/v1/leads` →
/// `("https", "api.example.com", "/v1/leads")`; a URL with no path component yields
/// an empty `host`-suffix path of `"/"`. Best-effort parsing for audit metadata
/// only (the SC-5 policy already validated the URL before the fetch ran), so a
/// malformed URL degrades to empty parts rather than erroring.
fn split_url(url: &str) -> (String, String, String) {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_string(), r),
        None => (String::new(), url),
    };
    match rest.split_once('/') {
        Some((host, path)) => (scheme, host.to_string(), format!("/{path}")),
        None => (scheme, rest.to_string(), "/".to_string()),
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
