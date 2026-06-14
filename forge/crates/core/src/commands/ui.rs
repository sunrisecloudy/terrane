//! `ui.dispatch_event` (the keystone interactive loop, UI-4/CR-6) and
//! `runtime.replay_session` (its session-replay analogue, UI-4/CR-8). Moved
//! verbatim from `workspace.rs` (/simplify #11a): the two handlers, the
//! renderer-facing error-code classifier ([`dispatch_error_code`]), the session
//! shape/patch-chain helpers, and their co-located unit tests.

use forge_domain::{AppletId, CoreError, Result, RunRecord};
use forge_runtime::{record_dispatch, Program as RuntimeProgram};

use crate::determinism::{derive_seeds, unique_run_id};
use crate::StorageHostBridge;

use super::super::{AppletLifecycle, WorkspaceCore};
use super::require_applet_id;

impl WorkspaceCore {
    /// `runtime.replay_session` — replay an ordered **event session** (an initial
    /// `runtime.run` record followed by N `ui.dispatch_event` records, in dispatch
    /// order) and prove the WHOLE sequence replays byte-identically (prd-merged/05
    /// UI-4, prd-merged/01 CR-6, CR-8). This is the session-level analogue of
    /// `runtime.replay`: where `runtime.replay` blesses ONE recorded run, this blesses
    /// a recorded interactive session as a unit, so a multi-event session round-trips.
    ///
    /// Payload: `{ run_ids: [ <initial run_id>, <event run_id>, ... ] }` — the
    /// session in dispatch order (the ids the initial `runtime.run` + each accepted
    /// `ui.dispatch_event` returned).
    ///
    /// For each id we replay the run via [`replay_run_by_id`](Self::replay_run_by_id)
    /// (which version-pins the program/manifest and asserts that single run is
    /// byte-identical), then we ALSO:
    ///   - re-derive each event's UI patch by diffing the replayed run's final tree
    ///     against the PRIOR run's final tree, exactly as the live `ui.dispatch_event`
    ///     loop did, and assert that re-derived patch equals the originally recorded
    ///     one (so every patch is byte-identical, not just the host-call trace);
    ///   - fold each record's per-run fingerprint into a composite
    ///     [`session_fingerprint`](RunRecord::session_fingerprint), and assert the
    ///     replayed session's composite equals the original's — which is sensitive to
    ///     BOTH per-run divergence AND event ORDER (two events applied in a different
    ///     order produce a different composite, so order is enforced).
    ///
    /// Divergence anywhere — a single run, a patch, or the composite — is a typed
    /// `RuntimeError`/`ValidationError`, never a panic. The recorded permission
    /// snapshot governs each replay (CR-9); the live bridge is never consulted.
    pub(in crate::workspace) fn cmd_runtime_replay_session(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let run_ids: Vec<String> = match cmd.payload.get("run_ids") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        CoreError::ValidationError(
                            "runtime.replay_session `run_ids` entries must be strings".into(),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(CoreError::ValidationError(
                    "runtime.replay_session requires a non-empty `run_ids` array".into(),
                ))
            }
        };
        if run_ids.is_empty() {
            return Err(CoreError::ValidationError(
                "runtime.replay_session `run_ids` must not be empty".into(),
            ));
        }

        // Replay each run in order, accumulating (original, replayed) pairs. We keep
        // BOTH chains so the byte-identity claim is derived AND checked here, not just
        // returned for a test to assert.
        let mut originals: Vec<RunRecord> = Vec::with_capacity(run_ids.len());
        let mut replayeds: Vec<RunRecord> = Vec::with_capacity(run_ids.len());
        let mut applet_id: Option<AppletId> = None;

        for run_id in &run_ids {
            let (original, replayed) = self.replay_run_by_id(run_id, &cmd.actor)?;
            // Every run in one session must belong to the SAME applet — a session is
            // one applet's interactive loop. A mixed-applet `run_ids` list is a
            // caller error, not a silent cross-applet replay.
            match &applet_id {
                None => applet_id = Some(replayed.applet_id.clone()),
                Some(id) if id != &replayed.applet_id => {
                    return Err(CoreError::ValidationError(format!(
                        "runtime.replay_session run {run_id} belongs to applet {} but the session started with {}",
                        replayed.applet_id, id
                    )));
                }
                Some(_) => {}
            }
            originals.push(original);
            replayeds.push(replayed);
        }

        // STRUCTURAL session-shape guard. The patch-chain walk treats `run_ids[0]` as
        // the session HEAD (the initial `runtime.run` whose render is only the diff
        // base) and `run_ids[1..]` as the dispatched EVENTS (each diffed against the
        // prior render). That contract is only meaningful for a well-formed session:
        // the head must be a non-dispatch run and every tail entry must be a
        // `ui.dispatch_event` run, with no duplicate ids. Without this guard a caller
        // could pass an arbitrary same-applet `run_ids` list (a dispatch at the head,
        // a `runtime.run` mid-session, a duplicated id) and still get a misleading
        // `replays_identically: true` with a bogus "converged final tree" — because
        // each run self-replays and the recorded/replayed walks are trivially equal.
        // Rejecting a malformed shape up front makes the convergence claim load-bearing.
        let original_refs: Vec<&RunRecord> = originals.iter().collect();
        assert_well_formed_session(&run_ids, &original_refs)?;

        // Derive the ordered per-event patch chain + final tree from BOTH the recorded
        // (`originals`) and the replayed (`replayeds`) record sequences, walking each
        // run's final render against the PRIOR run's render — the same diff base the
        // live `ui.dispatch_event` loop used (UI-4). The two walks must be byte-equal:
        // that is the real session byte-identity claim (recorded patches == replayed
        // patches == recorded final tree == replayed final tree).
        let replayed_refs: Vec<&RunRecord> = replayeds.iter().collect();
        let (recorded_patches, recorded_final) = derive_session_patch_chain(&original_refs)?;
        let (event_patches, replayed_final) = derive_session_patch_chain(&replayed_refs)?;

        // The composite session identity: fold each record's per-run fingerprint in
        // order. Equal composites ⇒ each run replayed byte-identically to its recorded
        // counterpart, in order. Divergence is a RuntimeError.
        let session_fingerprint = RunRecord::session_fingerprint(&replayed_refs);
        let runs_replay_identically =
            RunRecord::session_replays_identically(&original_refs, &replayed_refs);
        if !runs_replay_identically {
            return Err(CoreError::RuntimeError(format!(
                "session replay diverged from the recorded session ({} run(s); composite fingerprints differ)",
                run_ids.len()
            )));
        }
        // Beyond the per-run trace fingerprint, the OBSERVABLE session output — the
        // ordered patch chain and the converged final tree — must reproduce exactly.
        // This is checked server-side so a caller's `replays_identically: true` is a
        // load-bearing claim, not something only the test asserts.
        if event_patches != recorded_patches {
            return Err(CoreError::RuntimeError(format!(
                "session replay diverged: re-derived event patch chain ({} event(s)) differs from the recorded one",
                event_patches.len()
            )));
        }
        if replayed_final != recorded_final {
            return Err(CoreError::RuntimeError(
                "session replay diverged: re-derived final tree differs from the recorded one"
                    .to_string(),
            ));
        }
        let replays_identically = runs_replay_identically;

        if let Some(applet_id) = &applet_id {
            self.events.emit(
                Some(applet_id.clone()),
                "session.replayed",
                serde_json::json!({
                    "applet_id": applet_id,
                    "run_ids": run_ids,
                    "events": run_ids.len().saturating_sub(1),
                    "ok": true,
                }),
            );
        }

        Ok(serde_json::json!({
            "ok": true,
            "run_ids": run_ids,
            // The per-event re-derived UI patches (one list per dispatched event, in
            // order). Already asserted byte-equal to the recorded chain above, so a
            // caller receives the verified patch sequence.
            "event_patches": event_patches,
            // The session's final rendered tree (the last replayed render, `null` if
            // nothing rendered). Already asserted equal to the recorded final tree.
            "final_tree": replayed_final,
            "session_fingerprint": session_fingerprint,
            "replays_identically": replays_identically,
        }))
    }

    /// `ui.dispatch_event` — re-enter an installed applet's handler on a UI event
    /// and produce the next UI patch (prd-merged/05 UI-4, prd-merged/01 CR-6). This
    /// is the keystone interactive loop through the facade: a rendered control
    /// carried an `onTap`/`onChange` `ActionRef`; the renderer sends that ref back
    /// with an event payload; this command dispatches the handler exported under
    /// that name over the **same** QuickJS containment / capability gate / record
    /// path as `runtime.run`, captures the handler's new UI tree, DIFFS it against
    /// the applet's last-known tree to a patch, emits a `ui.patch` event, persists
    /// the new tree as the next diff base, saves the recorded run (with the event in
    /// its trace), and returns `{ action_ref, tree, patches }`.
    ///
    /// Payload: `{ applet_id, action_ref, event_payload? }`.
    ///
    /// Contract (T034 `forge/fixtures/ui-events`), each a typed rejection with the
    /// applet's state + last-known tree UNCHANGED:
    ///   - a **null/absent `action_ref`** (an event on a control with no handler) is
    ///     a safe no-op: `{ ignored: true, patches: [] }`, no dispatch (the
    ///     `no_handler_event_ignored` vector);
    ///   - a **suspended applet** is rejected BEFORE any handler runs with
    ///     `ValidationError("ui.applet_not_dispatchable: ... suspended")` (the
    ///     `suspended_applet_rejected` vector; the lifecycle is the trusted flag set
    ///     via [`set_applet_lifecycle`](Self::set_applet_lifecycle));
    ///   - an **unknown `action_ref`** (no such exported handler) is the engine's
    ///     typed `ValidationError` (the `unknown_action_rejected` vector);
    ///   - an **invalid payload** / a **throwing handler** surfaces the handler's
    ///     own typed error (a `ValidationError` the handler raised, or a
    ///     `RuntimeError` for an uncaught throw) — the run record captures the
    ///     failure (`handler_throws_prior_tree_intact` / `invalid_payload_rejected`).
    ///
    /// The realm is one-shot per dispatch, so a handler persists state ONLY through
    /// `ctx.db`/`ctx.storage`; the next event reads it back. Command-level RBAC
    /// (run-capable roles) gates entry; the applet's manifest capabilities gate each
    /// `ctx.*` call inside the handler, exactly as a run.
    pub(in crate::workspace) fn cmd_ui_dispatch_event(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        // The dispatch key (T034): the `ActionRef` the rendered control carried. A
        // null/absent ref means the targeted control has NO handler for this event —
        // a safe ignored no-op, not an error (the `no_handler_event_ignored` vector).
        let action_ref = match cmd.payload.get("action_ref") {
            None | Some(serde_json::Value::Null) => {
                return Ok(serde_json::json!({
                    "applet_id": applet_id,
                    "ignored": true,
                    "reason": "target node has no ActionRef for this event",
                    "patches": [],
                }));
            }
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(CoreError::ValidationError(format!(
                    "ui.dispatch_event `action_ref` must be a string, got {other}"
                )))
            }
        };
        let event_payload = cmd
            .payload
            .get("event_payload")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Lifecycle gate (UI-4): a suspended applet has no live UI session, so a UI
        // event is rejected BEFORE any handler runs and with NO state change (the
        // `suspended_applet_rejected` vector). The flag is the TRUSTED per-applet
        // lifecycle, read from workspace state — never from the request — so an
        // applet cannot un-suspend itself by sending an event. We emit a
        // `ui.dispatch_rejected` event carrying the renderer-facing code
        // (`ui.applet_not_dispatchable`) with `dispatch_attempted: false`, so the
        // pre-dispatch rejection is observable to a renderer/host exactly like the
        // post-dispatch failure below (T034 `dispatch_attempted` flag).
        if self.applet_lifecycle(applet_id.as_str())? == AppletLifecycle::Suspended {
            let error = CoreError::ValidationError(format!(
                "ui.applet_not_dispatchable: applet {applet_id} is suspended; UI events are rejected before dispatch"
            ));
            self.events.emit(
                Some(applet_id.clone()),
                "ui.dispatch_rejected",
                serde_json::json!({
                    "applet_id": applet_id,
                    "action_ref": action_ref,
                    "dispatch_attempted": false,
                    "code": dispatch_error_code(&error),
                    "message": error.to_string(),
                }),
            );
            return Err(error);
        }

        let installed = self.load_applet(applet_id.as_str())?.ok_or_else(|| {
            CoreError::ValidationError(format!("applet {applet_id} is not installed"))
        })?;

        // The DIFF BASE: the applet's last-known tree (the previous render this
        // facade saw). Absent (the applet has not rendered yet) ⇒ `None`, so the
        // first event's diff is a single root replace, exactly like `runtime.run`'s
        // first render (UI-1).
        let prev_tree = self.load_ui_tree(applet_id.as_str())?;

        self.events.emit(
            Some(applet_id.clone()),
            "ui.dispatch_started",
            serde_json::json!({
                "applet_id": applet_id,
                "action_ref": action_ref,
                "code_hash": installed.code_hash,
            }),
        );

        let program = RuntimeProgram::new(applet_id.clone(), installed.js_code.clone());
        if program.code_hash() != installed.code_hash {
            return Err(CoreError::RuntimeError(format!(
                "code_hash provenance broken: runtime {} != pipeline {}",
                program.code_hash(),
                installed.code_hash
            )));
        }

        // Deterministic seams derived from `(code_hash, action_ref+payload)` so a
        // re-dispatch of the SAME event reproduces the SAME seeded time/random
        // values — the event replays byte-identically (the `replay_determinism_
        // same_sequence` vector). Mint a unique per-execution run id like a run.
        let dispatch_input = serde_json::json!([action_ref, event_payload]);
        let (random_seed, time_start) = derive_seeds(&installed.code_hash, &dispatch_input);
        let invocation = self.next_run_counter()?;

        let http_client = (self.http_client_factory)();
        let secret_store = (self.secret_store_factory)();
        // Build this dispatch's `ctx.files` sandbox from the injected factory, EXACTLY
        // like `cmd_runtime_run` does (CR-3 / spec/files.md). Without this a UI event
        // handler calling `ctx.files.read`/`write` would fail closed even when the
        // manifest grants files and `runtime.run` works for the same applet — breaking
        // the "same engine/host path as a run" promise (UI-4) for interactive applets
        // with file-backed handler state (review 112). Default = empty (fail-closed).
        let file_system = (self.file_system_factory)();

        // Re-enter the handler over the SAME engine path as a run: record mode,
        // live Store-backed bridge, manifest-gated `ctx.*`. `record_dispatch` runs
        // the handler named `action_ref` and records the `ui.dispatch_event`
        // envelope so the event is part of the replayable trace (T034 "events ARE
        // recorded in the run record").
        let mut bridge = StorageHostBridge::with_http_client(
            &mut self.store,
            applet_id.as_str(),
            http_client,
        )
        .with_secret_store(secret_store)
        .with_file_system(file_system);
        let mut run = record_dispatch(
            &program,
            &installed.manifest,
            &cmd.actor,
            &action_ref,
            &event_payload,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // The handler's final rendered tree (its last `ui.render`), if any. Drain
        // before dropping the bridge so the `&mut Store` borrow is released.
        let final_render = bridge.ui_renders.last().map(|r| r.tree.clone());
        drop(bridge);

        run.run_id = unique_run_id(&run.code_hash, invocation);

        // A failed dispatch (unknown handler → ValidationError, a handler throw →
        // RuntimeError, an invalid payload the handler rejected) is a typed
        // rejection: persist the failed record (so the denial/throw is auditable +
        // replayable), emit a failure event, and surface the handler's error. The
        // last-known tree is NOT advanced — the applet's prior view stays the diff
        // base (the error vectors' "tree_unchanged").
        if let forge_domain::RunOutcome::Failed { error } = &run.outcome {
            let error = error.clone();
            self.store_run_program(run.run_id.as_str(), &installed)?;
            self.store_program(&installed)?;
            self.store.save_run(&run)?;
            // The event carries BOTH the typed `CoreError` (for transport/audit)
            // AND the renderer-facing T034 code (`ui.action_not_found` for an
            // unknown handler, `runtime.handler_error` for a handler throw), so a
            // host/renderer can react to the stable code without parsing the
            // English error text. `dispatch_attempted: true` — the handler ran (or
            // we tried to resolve it), unlike the pre-dispatch suspended rejection.
            self.events.emit(
                Some(applet_id.clone()),
                "ui.dispatch_failed",
                serde_json::json!({
                    "applet_id": applet_id,
                    "action_ref": action_ref,
                    "run_id": run.run_id,
                    "dispatch_attempted": true,
                    "code": dispatch_error_code(&error),
                    "message": error.to_string(),
                    "error": error,
                }),
            );
            return Err(error);
        }

        // The handler completed. Its new tree is the last render; if the handler
        // rendered nothing, the view is unchanged (an empty patch over the prior
        // tree). Diff the new tree against the last-known tree to the next patch.
        let new_tree = match &final_render {
            Some(tree) => forge_ui::from_str(&tree.to_string())?,
            None => match &prev_tree {
                Some(prev) => prev.clone(),
                None => {
                    // No prior tree and no render: nothing to diff against. This is a
                    // degenerate dispatch (a handler that neither renders nor had a
                    // prior view); treat it as an empty-patch no-op over an empty base.
                    self.store_run_program(run.run_id.as_str(), &installed)?;
                    self.store_program(&installed)?;
                    self.store.save_run(&run)?;
                    return Ok(serde_json::json!({
                        "applet_id": applet_id,
                        "action_ref": action_ref,
                        "run_id": run.run_id,
                        "tree": serde_json::Value::Null,
                        "patches": [],
                    }));
                }
            },
        };
        let patches = forge_ui::diff(prev_tree.as_ref(), &new_tree);
        let patches_json = serde_json::to_value(&patches).map_err(|e| {
            CoreError::ValidationError(format!("ui.dispatch_event patch serialize failed: {e}"))
        })?;
        let tree_json = serde_json::to_value(&new_tree).map_err(|e| {
            CoreError::ValidationError(format!("ui.dispatch_event tree serialize failed: {e}"))
        })?;

        // Persist the new tree as the next diff base BEFORE returning, so the next
        // event in the session diffs against this one (the loop's state link).
        self.store_ui_tree(applet_id.as_str(), &tree_json)?;

        // Pin the per-run replay artifact + persist the recorded run (event in the
        // trace) so the dispatch replays byte-identically, exactly like a run.
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;
        self.store.save_run(&run)?;

        // Emit the UI patch event — the link the renderer consumes to advance the
        // live tree (UI-1/UI-4).
        self.events.emit(
            Some(applet_id.clone()),
            "ui.patch",
            serde_json::json!({
                "applet_id": applet_id,
                "action_ref": action_ref,
                "run_id": run.run_id,
                "tree": tree_json,
                "patches": patches_json,
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "action_ref": action_ref,
            "run_id": run.run_id,
            "tree": tree_json,
            "patches": patches_json,
        }))
    }
}

/// Classify a `ui.dispatch_event` rejection into its **renderer-facing error
/// code** (the T034 `expect.results[i].error.code` space, `forge/fixtures/ui-
/// events`). The typed [`CoreError`] is the transport/RBAC error; this is the
/// stable, renderer-visible code a host surfaces to the UI so it can show the
/// right affordance without parsing English error text:
///
///   - `ui.applet_not_dispatchable` — the applet is suspended; the event was
///     rejected BEFORE any handler ran (the `suspended_applet_rejected` vector).
///     Marked by the `ui.applet_not_dispatchable:` prefix the lifecycle gate
///     writes.
///   - `ui.action_not_found` — no handler is exported under the dispatched
///     `ActionRef` (the `unknown_action_rejected` vector). The engine raises a
///     `ValidationError` whose message is `no UI handler registered for action
///     ref …` (engine.rs `Entry::resolve`); we key off that exact marker.
///   - `ui.invalid_event_payload` — the handler ran but rejected the event PAYLOAD
///     as malformed (the `invalid_payload_rejected` vector — a TextField `onChange`
///     whose `value` was not a string). A handler signals this by throwing an
///     `Error` whose message starts with the `invalid event payload` marker; the
///     engine surfaces every JS throw as a `RuntimeError`, so we key off that
///     marker to refine an otherwise-generic handler throw into the contract's
///     dedicated payload-validation code. This lets a renderer distinguish "your
///     input was bad" (re-prompt the field) from a general "the handler crashed".
///   - `runtime.handler_error` — the handler ran and threw for any OTHER reason
///     (the `handler_throws_prior_tree_intact` vector). Every uncaught JS throw is
///     a `RuntimeError` (engine.rs `classify_failure`); the handler's own message
///     (e.g. `boom`) rides along in `message`.
///
/// Anything else (a `PermissionDenied`/`ResourceLimitExceeded`/etc. — e.g. a
/// `ctx.*` call the manifest did not grant) keeps the typed error's own
/// [`code`](CoreError::code) so a capability/limit failure is never mislabeled as
/// a UI/handler error.
fn dispatch_error_code(error: &CoreError) -> &'static str {
    match error {
        CoreError::ValidationError(msg) if msg.contains("ui.applet_not_dispatchable") => {
            "ui.applet_not_dispatchable"
        }
        CoreError::ValidationError(msg) if msg.contains("no UI handler registered") => {
            "ui.action_not_found"
        }
        // A handler that threw with the `invalid event payload` marker is the
        // contract's payload-validation rejection, not a generic crash. Match
        // case-insensitively on the marker so the engine's `entrypoint threw: …`
        // wrapping (or a capitalized handler message) still classifies.
        CoreError::RuntimeError(msg)
            if msg.to_ascii_lowercase().contains("invalid event payload") =>
        {
            "ui.invalid_event_payload"
        }
        CoreError::RuntimeError(_) => "runtime.handler_error",
        other => other.code(),
    }
}

/// The final UI tree a (replayed) run rendered — the tree of its LAST recorded
/// `ui.render` call (`args = [tree]`), parsed as a [`forge_ui::Node`]. `None` when
/// the run rendered nothing (so the session's diff base does not advance, and an
/// event that renders nothing yields an empty patch). Used by the session-replay
/// path to walk the replayed trees and re-derive each event's UI patch (UI-4).
fn replayed_final_tree(run: &RunRecord) -> Result<Option<forge_ui::Node>> {
    let last_render = run
        .calls
        .iter()
        .rev()
        .find(|c| c.method == "ui.render")
        .and_then(|c| c.args.as_array().and_then(|a| a.first()).cloned());
    match last_render {
        Some(tree_json) => {
            let node = forge_ui::from_str(&tree_json.to_string())?;
            Ok(Some(node))
        }
        None => Ok(None),
    }
}

/// True iff `run` is a dispatched UI event (its recorded trace carries a
/// `ui.dispatch_event` envelope). The initial `runtime.run` that opens a session
/// has none; every `ui.dispatch_event` run has exactly one (recorder.rs). Used to
/// validate a replay session's SHAPE (head = a run, tail = events).
fn is_dispatch_run(run: &RunRecord) -> bool {
    run.calls.iter().any(|c| c.method == "ui.dispatch_event")
}

/// Reject a malformed `runtime.replay_session` `run_ids` list before the patch-chain
/// walk derives a (bogus) "converged" session. A well-formed session is exactly the
/// shape the live `ui.dispatch_event` loop produces and the walk assumes:
///   - `records[0]` is the session HEAD: the initial `runtime.run`, NOT a dispatch
///     (its render is only the diff base for event #1);
///   - every `records[1..]` entry is a dispatched event (a `ui.dispatch_event` run);
///   - no `run_id` appears twice (a session is a linear ordered trace, not a multiset
///     — a duplicate would double-apply one event's diff against itself).
///
/// Any violation is a typed `ValidationError` naming the offending id, so the
/// command's `replays_identically: true` / `final_tree` is a load-bearing claim about
/// a real recorded session, never an artifact of an arbitrary id list.
fn assert_well_formed_session(run_ids: &[String], records: &[&RunRecord]) -> Result<()> {
    // Linear trace: no duplicate ids. (`run_ids` and `records` are 1:1 by index.)
    for i in 0..run_ids.len() {
        for j in (i + 1)..run_ids.len() {
            if run_ids[i] == run_ids[j] {
                return Err(CoreError::ValidationError(format!(
                    "runtime.replay_session `run_ids` must be a linear session but run {} appears more than once",
                    run_ids[i]
                )));
            }
        }
    }
    // Head must be the opening run, not a dispatched event.
    if is_dispatch_run(records[0]) {
        return Err(CoreError::ValidationError(format!(
            "runtime.replay_session head run {} is a dispatched UI event, but a session must start with the initial runtime.run",
            run_ids[0]
        )));
    }
    // Every later entry must be a dispatched event (not another initial run spliced in).
    for (run_id, run) in run_ids[1..].iter().zip(&records[1..]) {
        if !is_dispatch_run(run) {
            return Err(CoreError::ValidationError(format!(
                "runtime.replay_session run {run_id} is not a dispatched UI event, but every run after the head must be a ui.dispatch_event"
            )));
        }
    }
    Ok(())
}

/// Walk an ordered **event session** (`records[0]` is the initial `runtime.run`,
/// `records[1..]` are the dispatched `ui.dispatch_event` runs in order) and derive
/// the OBSERVABLE session output the live `ui.dispatch_event` loop produced: the
/// ordered per-event UI patch chain and the converged final tree (UI-4).
///
/// Each event's patch is `forge_ui::diff(prior_render, this_render)` — diffing this
/// run's final render against the PRIOR run's render, the same diff base the live
/// loop used. A run that rendered nothing leaves the view unchanged: it contributes
/// an empty patch and does NOT advance the diff base (so the next event still diffs
/// against the last real render). The head run contributes no patch (its render is
/// only the base for event #1). Returns `(event_patches, final_tree_json)` where
/// `final_tree_json` is `null` if nothing rendered across the whole session.
///
/// Driving BOTH the recorded and the replayed record sequences through this single
/// walk and asserting the two outputs are byte-equal is the session byte-identity
/// check in [`cmd_runtime_replay_session`](WorkspaceCore::cmd_runtime_replay_session):
/// equal recorded/replayed walks ⇒ every patch and the final tree reproduced exactly.
fn derive_session_patch_chain(
    records: &[&RunRecord],
) -> Result<(Vec<serde_json::Value>, serde_json::Value)> {
    let mut prev_tree: Option<forge_ui::Node> = None;
    let mut event_patches: Vec<serde_json::Value> = Vec::new();
    for (step, run) in records.iter().enumerate() {
        let next_tree = replayed_final_tree(run)?;
        if step > 0 {
            // Every run after the head is a dispatched event. Diff its render against
            // the prior render to the event's patch; a non-rendering run is an empty
            // patch over the unchanged view.
            let patches = match &next_tree {
                Some(tree) => forge_ui::diff(prev_tree.as_ref(), tree),
                None => Vec::new(),
            };
            let patches_json = serde_json::to_value(&patches).map_err(|e| {
                CoreError::ValidationError(format!("replay_session patch serialize failed: {e}"))
            })?;
            event_patches.push(patches_json);
        }
        // Only advance the diff base when this run actually rendered, so a
        // non-rendering event does not blank out the prior tree.
        if next_tree.is_some() {
            prev_tree = next_tree;
        }
    }
    let final_tree = match prev_tree {
        Some(tree) => serde_json::to_value(&tree).map_err(|e| {
            CoreError::ValidationError(format!("replay_session final tree serialize failed: {e}"))
        })?,
        None => serde_json::Value::Null,
    };
    Ok((event_patches, final_tree))
}

#[cfg(test)]
mod session_patch_chain_tests {
    use super::*;
    use forge_domain::{AppResult, RecordedCall, RunOutcome};

    /// A minimal `RunRecord` whose only relevant trace is a single `ui.render` of
    /// `tree` (or no render at all when `tree` is `None`) — enough to exercise the
    /// session patch-chain walk without standing up the engine.
    fn rendered(tree: Option<serde_json::Value>) -> RunRecord {
        let calls = match tree {
            Some(t) => vec![RecordedCall {
                seq: 0,
                method: "ui.render".into(),
                args: serde_json::json!([t]),
                response: serde_json::json!(null),
            }],
            None => Vec::new(),
        };
        RunRecord {
            run_id: forge_domain::RunId::new("r"),
            applet_id: AppletId::new("app"),
            code_hash: forge_domain::hash::code_hash("body"),
            input: serde_json::json!(null),
            random_seed: 0,
            time_start: 0,
            calls,
            logs: Vec::new(),
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: serde_json::json!(null) },
            },
        }
    }

    fn text(t: &str) -> serde_json::Value {
        serde_json::json!({ "type": "Text", "testId": "t", "text": t })
    }

    /// Like [`rendered`] but with a `ui.dispatch_event` envelope appended — the trace
    /// shape of an accepted `ui.dispatch_event` run (a session EVENT, not the head).
    /// `id` lets a test give each run a distinct id to exercise the duplicate guard.
    fn dispatched(id: &str, tree: Option<serde_json::Value>) -> RunRecord {
        let mut run = rendered(tree);
        run.run_id = forge_domain::RunId::new(id);
        run.calls.push(RecordedCall {
            seq: run.calls.len() as u64,
            method: "ui.dispatch_event".into(),
            args: serde_json::json!(["step", {}]),
            response: serde_json::json!(null),
        });
        run
    }

    /// A head run (an initial `runtime.run`: a plain render, no dispatch envelope)
    /// with a distinct id.
    fn head(id: &str, tree: Option<serde_json::Value>) -> RunRecord {
        let mut run = rendered(tree);
        run.run_id = forge_domain::RunId::new(id);
        run
    }

    /// A well-formed session (head run + dispatched events, distinct ids) is accepted.
    #[test]
    fn well_formed_session_is_accepted() {
        let ids = vec!["h".into(), "e1".into(), "e2".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let e2 = dispatched("e2", Some(text("c")));
        assert_well_formed_session(&ids, &[&h, &e1, &e2]).unwrap();
    }

    /// A single-run session (just the head) is well-formed.
    #[test]
    fn single_head_only_session_is_well_formed() {
        let ids = vec!["h".into()];
        let h = head("h", Some(text("a")));
        assert_well_formed_session(&ids, &[&h]).unwrap();
    }

    /// A dispatched event at the HEAD is rejected: a session must open with the
    /// initial `runtime.run`, not a `ui.dispatch_event`.
    #[test]
    fn dispatch_at_head_is_rejected() {
        let ids = vec!["e0".into(), "e1".into()];
        let e0 = dispatched("e0", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let err = assert_well_formed_session(&ids, &[&e0, &e1]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("head run e0"), "{err}");
    }

    /// A non-dispatch run spliced into the TAIL is rejected: every run after the head
    /// must be a dispatched event, not a second initial run.
    #[test]
    fn non_dispatch_in_tail_is_rejected() {
        let ids = vec!["h".into(), "e1".into(), "h2".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let h2 = head("h2", Some(text("c"))); // a runtime.run spliced mid-session
        let err = assert_well_formed_session(&ids, &[&h, &e1, &h2]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("run h2"), "{err}");
    }

    /// A duplicated run id is rejected: a session is a linear ordered trace, not a
    /// multiset (a duplicate would double-apply one event's diff against itself).
    #[test]
    fn duplicate_run_id_is_rejected() {
        let ids = vec!["h".into(), "e1".into(), "e1".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let e1b = dispatched("e1", Some(text("c")));
        let err = assert_well_formed_session(&ids, &[&h, &e1, &e1b]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("appears more than once"), "{err}");
    }

    /// The head run contributes NO patch (its render is only the diff base); each
    /// subsequent run is an event whose patch diffs its render against the prior
    /// render. The final tree is the last render.
    #[test]
    fn head_is_base_and_events_diff_against_prior_render() {
        let records = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        let (patches, final_tree) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches.len(), 1, "one event after the head");
        let want = forge_ui::diff(
            Some(&forge_ui::from_str(&text("a").to_string()).unwrap()),
            &forge_ui::from_str(&text("b").to_string()).unwrap(),
        );
        assert_eq!(patches[0], serde_json::to_value(&want).unwrap());
        assert_eq!(final_tree, text("b"));
    }

    /// A non-rendering event contributes an EMPTY patch and does NOT advance the
    /// diff base, so the NEXT event still diffs against the last real render.
    #[test]
    fn non_rendering_event_is_empty_patch_and_does_not_advance_base() {
        let records = [
            &rendered(Some(text("a"))),
            &rendered(None),           // event #1 renders nothing
            &rendered(Some(text("c"))), // event #2 diffs c against a, not against "nothing"
        ];
        let (patches, final_tree) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0], serde_json::json!([]), "non-rendering event = empty patch");
        let want = forge_ui::diff(
            Some(&forge_ui::from_str(&text("a").to_string()).unwrap()),
            &forge_ui::from_str(&text("c").to_string()).unwrap(),
        );
        assert_eq!(patches[1], serde_json::to_value(&want).unwrap(), "next event still diffs against \"a\"");
        assert_eq!(final_tree, text("c"));
    }

    /// An identical re-render is an EMPTY patch (no spurious diff).
    #[test]
    fn identical_rerender_is_empty_patch() {
        let records = [&rendered(Some(text("same"))), &rendered(Some(text("same")))];
        let (patches, _) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches[0], serde_json::json!([]));
    }

    /// The walk is ORDER-sensitive: swapping two distinct events yields a different
    /// patch chain and a different final tree — the property the command relies on
    /// to enforce recorded event order.
    #[test]
    fn walk_is_order_sensitive() {
        let head = rendered(Some(text("a")));
        let e_b = rendered(Some(text("b")));
        let e_c = rendered(Some(text("c")));
        let (ordered, ordered_final) =
            derive_session_patch_chain(&[&head, &e_b, &e_c]).unwrap();
        let (swapped, swapped_final) =
            derive_session_patch_chain(&[&head, &e_c, &e_b]).unwrap();
        assert_ne!(ordered, swapped, "swapped order = different patch chain");
        assert_ne!(ordered_final, swapped_final, "swapped order = different final tree");
    }

    /// Two byte-identical record sequences produce byte-identical chains — the
    /// equality the command asserts between the recorded and replayed walks. This
    /// is the building block of the server-side `replays_identically` claim.
    #[test]
    fn identical_record_sequences_produce_identical_chains() {
        let a = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        let b = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        assert_eq!(
            derive_session_patch_chain(&a).unwrap(),
            derive_session_patch_chain(&b).unwrap()
        );
    }

    /// A single-run session (just the head) has no events: an empty patch chain and
    /// the head's render as the final tree.
    #[test]
    fn single_run_session_has_no_event_patches() {
        let (patches, final_tree) =
            derive_session_patch_chain(&[&rendered(Some(text("only")))]).unwrap();
        assert!(patches.is_empty());
        assert_eq!(final_tree, text("only"));
    }
}

#[cfg(test)]
mod dispatch_error_code_tests {
    use super::*;

    // Pin the T034 renderer-facing classification (`forge/fixtures/ui-events`)
    // independent of the JS engine path: each rejection family maps to the stable
    // code a renderer keys on, and an unrelated typed error keeps its own code.

    #[test]
    fn suspended_gate_maps_to_applet_not_dispatchable() {
        let e = CoreError::ValidationError(
            "ui.applet_not_dispatchable: applet x is suspended; UI events are rejected before dispatch".into(),
        );
        assert_eq!(dispatch_error_code(&e), "ui.applet_not_dispatchable");
    }

    #[test]
    fn unknown_handler_maps_to_action_not_found() {
        // The engine raises exactly this message for a missing handler
        // (engine.rs `Entry::resolve`); the classifier keys off the marker.
        let e = CoreError::ValidationError(
            "no UI handler registered for action ref \"counter.delete_everything\"".into(),
        );
        assert_eq!(dispatch_error_code(&e), "ui.action_not_found");
    }

    #[test]
    fn handler_throw_maps_to_runtime_handler_error() {
        // A generic uncaught JS throw (no marker) is a `runtime.handler_error`.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError("boom".into())),
            "runtime.handler_error"
        );
        // Even when wrapped by the engine's `entrypoint threw: …` prefix.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError("entrypoint threw: boom".into())),
            "runtime.handler_error"
        );
    }

    #[test]
    fn invalid_payload_throw_maps_to_invalid_event_payload() {
        // A handler that threw with the `invalid event payload` marker is the
        // contract's dedicated payload-validation code, NOT a generic crash —
        // so a renderer can re-prompt the field instead of showing a fatal error.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError(
                "invalid event payload: value must be a string".into()
            )),
            "ui.invalid_event_payload"
        );
        // The marker still classifies through the engine's `entrypoint threw: …`
        // wrapping and is matched case-insensitively.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError(
                "entrypoint threw: Error: Invalid Event Payload: value must be a string".into()
            )),
            "ui.invalid_event_payload"
        );
    }

    #[test]
    fn capability_or_limit_failure_keeps_its_own_code() {
        // A `ctx.*` call the manifest did not grant must NOT be relabeled as a
        // UI/handler error — it keeps its typed code so an authz/limit failure
        // stays distinguishable from a missing handler or a handler throw.
        assert_eq!(
            dispatch_error_code(&CoreError::PermissionDenied("storage.set".into())),
            "PermissionDenied"
        );
        assert_eq!(
            dispatch_error_code(&CoreError::ResourceLimitExceeded("fuel".into())),
            "ResourceLimitExceeded"
        );
        // A non-marked ValidationError (not a UI dispatch marker) keeps its kind.
        assert_eq!(
            dispatch_error_code(&CoreError::ValidationError("applet x is not installed".into())),
            "ValidationError"
        );
    }
}
