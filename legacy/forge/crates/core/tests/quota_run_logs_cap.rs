//! DL-22 review 178 LIVE-WIRING proofs that the `run_logs` cap is enforced as a
//! PRE-FLIGHT ADMISSION gate — a run that cannot be admitted is rejected BEFORE any
//! applet side effect runs, so it leaves NO new records and NO new UI state behind — and
//! that an ADMITTED run ALWAYS persists its mandatory run record (CR-9), never dropped
//! post-execution.
//!
//! P1 (review 178) — UNREPLAYABLE SIDE EFFECTS. Each `ctx.db` write an applet makes
//! commits to SQLite IMMEDIATELY as the applet runs (`apply_mutation_crdt`), so the
//! applet's record writes are durable the instant it executes. The review-177 fix gated
//! the run record AFTER the applet ran (`save_run_with_quota_tx`), so a rejection there
//! left committed applet writes with NO run record to replay from — a CR-9 violation
//! (every execution persists its resulting writes) and torn, unreplayable state. The fix
//! moves the run_logs + workspace-total check to a PRE-FLIGHT ADMISSION gate
//! (`Store::admit_run_or_reject`) that runs BEFORE the applet/handler/callback: a
//! workspace whose run-log budget is exhausted REFUSES to START new runs
//! (reject-not-delete), so a rejection commits NOTHING. Once admitted, the run record
//! ALWAYS persists.
//!
//! This applies to all three run-persistence paths: `runtime.run`, `ui.dispatch_event`,
//! and the `db.watch` callback.
//!
//! P2 — `quota.status.approaching` lists ALL budgets at/above the threshold (review 177,
//! retained here as a regression).

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};
use forge_storage::{AuditQuery, IndexManager, Mutation, QuotaCategory, QuotaPolicy};
use serde_json::{json, Value};

/// A demo applet whose `main` inserts a `tasks` record (so `runtime.run` commits an
/// applet write AND persists a run record) and whose `bump` UI handler writes a `tasks`
/// record then re-renders (so a `ui.dispatch_event` commits an applet write too — the
/// write that would be stranded if a post-execution gate rejected the run record).
const DEMO_TS: &str = r#"
    export async function main(ctx, input) {
        const title = (input && input.title) ? input.title : "untitled";
        await ctx.db.insert("tasks", { title: title, done: false });
        ctx.ui.render({ type: "Text", text: title });
        return { ok: true, value: { ok: true } };
    }
    export const handlers = {
        "bump": async (ctx, _event) => {
            await ctx.db.insert("tasks", { title: "bumped", done: false });
            ctx.ui.render({ type: "Text", text: "bumped" });
            return { ok: true, value: { bumped: true } };
        },
    };
"#;

/// A watching applet: `main` registers a `db.watch` over open tasks and `onWatch`
/// writes a `tasks` record then re-renders. Its callback re-entry would commit an applet
/// write AND persist a run record (the third path) — the write a post-execution gate
/// would strand.
const WATCH_TS: &str = r#"
    export async function main(ctx, input) {
        await ctx.db.watch("watch:open", { from: "tasks", where: ["done", "=", false], orderBy: ["id", "asc"] });
        return { ok: true, value: { registered: true } };
    }
    export async function onWatch(ctx, n) {
        const ids = (n && n.result_ids) ? n.result_ids : [];
        await ctx.db.insert("tasks", { title: "callback wrote", done: true });
        await ctx.ui.render({ type: "Text", text: "open=" + ids.length });
        return { ok: true, value: { count: ids.length } };
    }
"#;

fn manifest() -> Value {
    json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "db": { "read": ["tasks"], "write": ["tasks"] },
            "storage": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    })
}

fn owner() -> ActorContext {
    ActorContext::owner("dev")
}

fn cmd(name: &str, applet_id: Option<&str>, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor: owner(),
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: applet_id.map(AppletId::new),
        name: name.into(),
        payload,
    }
}

fn install(core: &mut WorkspaceCore, applet_id: &str, src: &str) {
    let resp = core.handle(cmd(
        "applet.install",
        Some(applet_id),
        json!({ "manifest": manifest(), "sources": { "src/main.ts": src } }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);
}

fn run_with_title(core: &mut WorkspaceCore, applet_id: &str, title: &str) -> forge_domain::CoreResponse {
    core.handle(cmd("runtime.run", Some(applet_id), json!({ "input": { "title": title } })))
}

fn run_logs_bytes(core: &WorkspaceCore) -> u64 {
    core.quota_usage().unwrap().category_bytes(QuotaCategory::RunLogs)
}

/// The number of `tasks` records committed (the applet's `ctx.db` writes).
fn record_count(core: &WorkspaceCore) -> usize {
    core.store().list_records("tasks").unwrap().len()
}

/// The number of persisted run records (the CR-9 replay source). A pre-flight rejection
/// must leave this unchanged.
fn run_record_count(core: &WorkspaceCore) -> i64 {
    core.store()
        .connection()
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .unwrap()
}

/// Persist the current effective policy with `run_logs_cap` pinned to `cap` (the
/// trusted seam — never the write being checked).
fn pin_run_logs_cap(core: &mut WorkspaceCore, cap: u64) {
    let mut policy = core.quota_policy().unwrap();
    policy.run_logs_cap = cap;
    core.set_quota_policy(&policy).unwrap();
}

/// The typed-error shape a run-admission REJECTION surfaces on a `runtime.run` /
/// `ui.dispatch_event` response: the command returns Ok with `run_ok=false` and a
/// `ResourceLimitExceeded` result error... OR the command itself errors. This pulls the
/// error code + detail out of EITHER shape.
fn rejection(resp: &forge_domain::CoreResponse) -> Option<(String, String)> {
    if let Some(err) = &resp.error {
        return Some((err.code().to_string(), err.to_string()));
    }
    let e = &resp.payload["result"]["error"];
    let kind = e["kind"].as_str()?;
    let detail = e["detail"].as_str().unwrap_or_default();
    Some((kind.to_string(), detail.to_string()))
}

/// P1 (review 178): after tightening `run_logs_cap` so the workspace has NO run-log
/// headroom, a `runtime.run` and a `ui.dispatch_event` are each REJECTED PRE-FLIGHT and
/// leave NO new records and NO new run records behind — the applet's `ctx.db` write never
/// happens (no unreplayable side effects).
#[test]
fn run_logs_cap_rejects_runtime_run_and_ui_dispatch_preflight_with_no_side_effects() {
    let mut core = WorkspaceCore::in_memory("ws-rl").unwrap();
    install(&mut core, "app", DEMO_TS);

    // Seed a few runs so run_logs is non-trivial, and establish a diff base + handler
    // session for the UI dispatch path (each seed commits a tasks record + a run record).
    for i in 0..3 {
        let resp = run_with_title(&mut core, "app", &format!("seed {i}"));
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "seed run must succeed");
    }

    // Pin the run_logs cap to EXACTLY the current run_logs bytes: zero headroom, so a NEW
    // run cannot be ADMITTED. (Other budgets stay roomy — this isolates the run_logs cap
    // as the binding admission limit.)
    let cap = run_logs_bytes(&core);
    pin_run_logs_cap(&mut core, cap);

    // Capture the pre-rejection state: the run-log usage, the committed tasks records, and
    // the persisted run records. A PRE-FLIGHT rejection must leave ALL THREE unchanged.
    let records_before = record_count(&core);
    let runs_before = run_record_count(&core);

    // runtime.run: the workspace has no run-log budget to admit the run → REJECTED BEFORE
    // `main` runs. The applet's `ctx.db.insert` never happens; no run record is written.
    core.events_mut().drain();
    let resp = run_with_title(&mut core, "app", "this run is not admitted");
    let (code, detail) = rejection(&resp).expect("runtime.run must be rejected pre-flight over the run_logs cap");
    assert_eq!(code, "ResourceLimitExceeded", "runtime.run rejection is a quota error: {resp:?}");
    assert!(detail.contains("no data was deleted"), "carries the cleanup/export suggestion: {detail}");
    assert!(detail.contains("run_logs") || detail.contains("category"), "names the run_logs budget: {detail}");
    // NO unreplayable side effects: the applet's record write never landed and no run
    // record was persisted (the rejection ran before any applet effect).
    assert_eq!(record_count(&core), records_before, "a rejected runtime.run commits NO applet record write");
    assert_eq!(run_record_count(&core), runs_before, "a rejected runtime.run persists NO run record");
    assert_eq!(run_logs_bytes(&core), cap, "run_logs usage is unchanged by the pre-flight rejection");

    // A `runtime.run.rejected` audit event marks the pre-flight denial (observable, like
    // the lifecycle gates).
    let events = core.events_mut().drain();
    assert!(
        events.iter().any(|e| e.kind == "runtime.run.rejected"
            && e.payload["error_code"] == json!("ResourceLimitExceeded")),
        "the pre-flight admission denial emits runtime.run.rejected: {events:?}"
    );

    // ui.dispatch_event: a re-render handler that would also commit a `ctx.db` write +
    // persist a run record is rejected the same way, BEFORE the handler runs.
    let records_before = record_count(&core);
    let runs_before = run_record_count(&core);
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("app"),
        json!({ "action_ref": "bump", "event_payload": {} }),
    ));
    let (dcode, ddetail) = rejection(&dispatch).expect("ui.dispatch_event must be rejected pre-flight over the run_logs cap");
    assert_eq!(dcode, "ResourceLimitExceeded", "ui.dispatch_event rejection is a quota error: {dispatch:?}");
    assert!(ddetail.contains("no data was deleted"), "carries the cleanup/export suggestion: {ddetail}");
    // NO unreplayable side effects: the handler's `ctx.db` write never landed and no run
    // record was persisted (the dispatch was rejected before the handler ran).
    assert_eq!(record_count(&core), records_before, "a rejected ui.dispatch_event commits NO handler record write");
    assert_eq!(run_record_count(&core), runs_before, "a rejected ui.dispatch_event persists NO run record");
    assert_eq!(run_logs_bytes(&core), cap, "run_logs usage still unchanged after the dispatch rejection");
}

/// P1 (review 179): a downstream `db.watch` callback whose re-entry cannot be admitted
/// over the tightened `run_logs_cap` is SKIPPED, NOT failed — the UPSTREAM producer (here
/// the triggering `commit_and_notify` mutation) stays SUCCESSFUL because its own durable
/// effects already committed, and the skipped callback is RECORDED as a
/// `db.watch.callback_rejected` decision (replay reproduces the identical skip). The
/// callback never runs (no `ctx.db` write) and no callback run record is written.
///
/// This REPLACES the review-178 behavior where the callback admission denial BUBBLED OUT
/// of the producer, reporting a failed run/write whose triggering side effects in fact
/// landed.
#[test]
fn run_logs_cap_skips_watch_callback_without_failing_producer() {
    let mut core = WorkspaceCore::in_memory("ws-rl-watch").unwrap();
    install(&mut core, "watcher", WATCH_TS);

    // Register the watch (its own run record persists fine under the default cap).
    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "watch registration run must succeed: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

    // Pin run_logs to zero headroom so the NEXT run (the callback re-entry) cannot be
    // admitted.
    let cap = run_logs_bytes(&core);
    pin_run_logs_cap(&mut core, cap);

    let runs_before = run_record_count(&core);

    // A committed mutation that enters the watched result would dispatch the onWatch
    // callback; the callback re-entry cannot be admitted over the run_logs cap. The
    // PRODUCER mutation (`tasks/1`) still commits and `commit_and_notify` SUCCEEDS — the
    // callback delivery is recorded as SKIPPED, not bubbled out as an error.
    let insert = Mutation::Insert {
        collection: "tasks".into(),
        id: Some("tasks/1".into()),
        fields: json!({ "title": "open task", "done": false }).as_object().unwrap().clone(),
        logical_at: Some(10),
    };
    let batch = core.commit_and_notify(&insert).expect(
        "the producer mutation must SUCCEED — a downstream callback admission denial is a \
         skipped delivery, not a producer failure",
    );

    // The producer's own write landed (its durable effect committed).
    let producer_wrote = core
        .store()
        .list_records("tasks")
        .unwrap()
        .into_iter()
        .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("open task"));
    assert!(producer_wrote, "the triggering producer write commits even when the callback is skipped");

    // The callback delivery was SKIPPED (recorded), not run: the watch fired (notification
    // delivered + recorded in the pure notification stream), but no callback ran.
    assert!(batch.callback_runs.is_empty(), "a skipped callback never produces a callback run id");
    // The skip is a RECORDED decision (a `db.watch.callback_rejected` envelope) so replay
    // reproduces the identical skipped-callback outcome. It rides its OWN field, NOT the
    // notification `recorded_calls` stream (which stays a pure db.watch.notification
    // sequence).
    assert_eq!(batch.rejected_callbacks.len(), 1, "exactly one over-cap callback delivery was skipped");
    let recorded_skip = &batch.rejected_callbacks[0];
    assert_eq!(recorded_skip.method, "db.watch.callback_rejected", "the skip is recorded as a db.watch.callback_rejected decision");
    assert_eq!(recorded_skip.args["watch_id"], json!("watch:open"), "the recorded skip names the watch");
    assert_eq!(recorded_skip.args["reason"], json!("run_logs_cap"), "the recorded skip names the run_logs cap");
    assert_eq!(recorded_skip.response, json!({ "delivered": false }), "a skipped callback was NOT delivered");
    // The notification stream itself carries ONLY db.watch.notification envelopes (the
    // skip is NOT folded in), so replay_notification_stream stays well-formed.
    assert!(
        batch.recorded_calls.iter().all(|c| c.method == "db.watch.notification"),
        "the recorded notification stream stays a pure db.watch.notification sequence"
    );

    // NO unreplayable callback side effects: the callback's "callback wrote" record never
    // landed, and no callback run record was persisted (the callback never ran).
    let callback_wrote = core
        .store()
        .list_records("tasks")
        .unwrap()
        .into_iter()
        .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
    assert!(!callback_wrote, "a skipped watch callback commits NO `ctx.db` write");
    assert_eq!(run_record_count(&core), runs_before, "a skipped watch callback persists NO run record");
    assert!(run_logs_bytes(&core) <= cap, "run_logs usage never exceeds the cap when the callback is skipped");
}

/// P2 (review 179): a NEAR-CAP `ui.dispatch_event` whose handler write enters a watched
/// result admits its OWN (producer) run record, but the downstream watcher CALLBACK is
/// then SKIPPED over the cap — so `run_logs` ends at most ONE record past the cap (the
/// mandatory dispatch record), NOT two (the dispatch record AND a callback record both
/// admitted off the stale pre-dispatch usage). The next run is then rejected pre-flight.
///
/// Before the fix the dispatch run record was assigned/saved AFTER notifications were
/// delivered, so a watcher callback's admission read the PRE-dispatch `run_logs` value and
/// passed alongside the dispatch record → TWO records beyond the cap. The fix saves the
/// dispatch (producer) record BEFORE callback admission (mirroring `runtime.run`'s order),
/// so the callback admission counts against the already-saved dispatch record and is
/// skipped.
#[test]
fn ui_dispatch_with_watch_overshoots_run_logs_by_at_most_one_record() {
    let mut core = WorkspaceCore::in_memory("ws-rl-overshoot").unwrap();
    // `app` carries the `bump` handler (its `ctx.db.insert` of a `done:false` task ENTERS
    // the watched result); `watcher` owns the `done = false` watch + onWatch callback.
    install(&mut core, "app", DEMO_TS);
    install(&mut core, "watcher", WATCH_TS);

    // Establish `app`'s render base (so `ui.dispatch_event` has a diff base / session) and
    // register the watch.
    let main_resp = run_with_title(&mut core, "app", "base");
    assert!(main_resp.ok && main_resp.payload["ok"] == Value::Bool(true), "app main must establish a render base");
    let watch_resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(watch_resp.ok, "watch registration run must succeed: {:?}", watch_resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

    // Pin the run_logs cap to ONE byte over the current usage: the NEXT run (the dispatch)
    // is admitted (usage < cap), but once its mandatory run record lands, usage > cap, so a
    // watcher callback admitted DURING notification delivery is refused — UNLESS the
    // dispatch record were counted after the callback (the bug), in which case BOTH would
    // be admitted off the stale usage.
    let cap = run_logs_bytes(&core) + 1;
    pin_run_logs_cap(&mut core, cap);

    let runs_before = run_record_count(&core);
    core.events_mut().drain();

    // The dispatch's `bump` handler inserts a `done:false` task that ENTERS the watch, so a
    // notification fires and would re-enter the watcher's onWatch callback. The dispatch is
    // admitted; its run record is saved BEFORE the callback admission, so the callback is
    // SKIPPED (recorded), not admitted.
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("app"),
        json!({ "action_ref": "bump", "event_payload": {} }),
    ));
    assert!(dispatch.ok, "the near-cap ui.dispatch_event itself is admitted and succeeds: {dispatch:?}");

    // EXACTLY ONE new run record landed — the producer dispatch record. The watcher
    // callback was skipped (no second admitted run record), so the cap overshoots by at
    // most ONE record, not two.
    assert_eq!(
        run_record_count(&core),
        runs_before + 1,
        "the dispatch admits ONE producer record; the over-cap watcher callback is skipped (NOT a second admitted record)"
    );
    // The callback's `ctx.db` write never landed (it never ran).
    let callback_wrote = core
        .store()
        .list_records("tasks")
        .unwrap()
        .into_iter()
        .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
    assert!(!callback_wrote, "the skipped watcher callback commits NO `ctx.db` write");
    // The skip is observable as a `db.watch.callback_rejected` event (the recorded decision
    // also rides the delivered batch's recorded calls for replay).
    let events = core.events_mut().drain();
    assert!(
        events.iter().any(|e| e.kind == "db.watch.callback_rejected"
            && e.payload["watch_id"] == json!("watch:open")
            && e.payload["reason"] == json!("run_logs_cap")),
        "the skipped over-cap callback emits db.watch.callback_rejected: {events:?}"
    );

    // The NEXT run is now rejected PRE-FLIGHT — usage is one record past the cap, so the
    // admission gate refuses a fresh run (the overshoot stays bounded at one record).
    let next = run_with_title(&mut core, "app", "next");
    let (code, detail) = rejection(&next).expect("the next run must be rejected pre-flight over the run_logs cap");
    assert_eq!(code, "ResourceLimitExceeded", "the next run's rejection is a quota error: {next:?}");
    assert!(detail.contains("no data was deleted"), "carries the cleanup/export suggestion: {detail}");
    assert_eq!(run_record_count(&core), runs_before + 1, "the rejected next run persists NO further run record — overshoot stays at one");
}

/// A run ADMITTED with headroom ALWAYS persists its run record — the mandatory CR-9
/// record is never dropped post-execution. This is the other half of the review-178
/// contract: admission only refuses to START; an admitted run records.
#[test]
fn admitted_run_always_persists_its_run_record() {
    let mut core = WorkspaceCore::in_memory("ws-rl-ok").unwrap();
    install(&mut core, "app", DEMO_TS);

    let runs_before = run_record_count(&core);
    let records_before = record_count(&core);

    // Roomy budget ⇒ admitted. The run executes, commits its applet write, AND persists
    // its run record.
    let resp = run_with_title(&mut core, "app", "admitted");
    assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "an admitted run completes: {resp:?}");
    let run_id = resp.payload["run_id"].as_str().expect("an admitted run returns a run_id");

    assert_eq!(run_record_count(&core), runs_before + 1, "an admitted run persists exactly one run record");
    assert_eq!(record_count(&core), records_before + 1, "an admitted run commits its applet write");
    // The run record is loadable (the replay source actually landed) — never dropped.
    assert!(core.store().load_run(run_id).unwrap().is_some(), "the admitted run's record is the durable replay source");
}

// ---------------------------------------------------------------------------
// review 183 (P1): the over-cap skipped-callback decision must be DURABLE for the
// REAL command paths (runtime.run, ui.dispatch_event, time-travel restore), which
// DISCARD the returned `DeliveredBatch`. Before the fix the skip rode ONLY the
// in-memory batch + a transient `db.watch.callback_rejected` event, so the decision
// was LOST the moment the command returned / on restart — even though
// `forge/spec/quotas.md` makes it a RECORDED decision replay must reproduce. The fix
// persists one `watch.callback_rejected` deny row into the SC-12 durable append-only
// audit log per skipped callback. These tests query that durable log (reopening the
// file-backed store to prove restart-survival) AFTER the producer command discarded
// its batch, and assert the row is present with the right actor/watch/reason.
// ---------------------------------------------------------------------------

/// The durable SC-12 audit rows recording an over-cap watch-callback skip
/// (`action = watch.callback_rejected`, ordered by seq).
fn durable_skip_rows(core: &WorkspaceCore) -> Vec<forge_storage::AuditRecord> {
    core.store()
        .query_audit(&AuditQuery::by_action("watch.callback_rejected"))
        .unwrap()
}

/// Pin the run_logs cap so the producer command is ADMITTED (its mandatory record
/// lands, pushing usage over the cap) but the DOWNSTREAM watcher callback re-entered
/// during notification delivery is then SKIPPED over the cap (mirrors the review-179
/// near-cap overshoot tests). `cap = current + headroom`.
fn pin_one_record_headroom(core: &mut WorkspaceCore) {
    let cap = run_logs_bytes(core) + 1;
    pin_run_logs_cap(core, cap);
}

/// P1 (review 183): a `runtime.run` whose `main` write ENTERS a watched result admits
/// its own producer record, then SKIPS the over-cap watcher callback — and the skip is
/// persisted into the DURABLE audit log, surviving the discarded batch AND a store
/// REOPEN (restart). `runtime.run` discards the `DeliveredBatch`, so the durable row is
/// the only thing that carries the decision past command-return.
#[test]
fn runtime_run_over_cap_callback_skip_survives_in_durable_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws.forge");
    {
        let mut core = WorkspaceCore::open(&path, "ws-183-run").unwrap();
        // `app`'s `main` inserts a `done:false` task (ENTERS the watch); `watcher` owns the
        // `done = false` watch + onWatch callback whose re-entry would commit a write.
        install(&mut core, "app", DEMO_TS);
        install(&mut core, "watcher", WATCH_TS);
        let watch_resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
        assert!(watch_resp.ok, "watch registration run must succeed: {:?}", watch_resp.error);
        assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

        // ONE record of headroom: the runtime.run producer record is admitted (then over
        // cap), so the downstream onWatch callback admission is refused → skipped.
        pin_one_record_headroom(&mut core);

        // The producer run is admitted and SUCCEEDS; its returned batch is DISCARDED by the
        // command path. Pre-fix, that discard lost the skip decision.
        let resp = run_with_title(&mut core, "app", "open task");
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "the producer runtime.run is admitted and succeeds: {resp:?}");
        // The over-cap watcher callback never ran (no `ctx.db` write landed).
        let callback_wrote = core
            .store()
            .list_records("tasks")
            .unwrap()
            .into_iter()
            .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
        assert!(!callback_wrote, "the over-cap watcher callback commits NO `ctx.db` write");

        // The DURABLE decision is present even though the batch was discarded.
        let rows = durable_skip_rows(&core);
        assert_eq!(rows.len(), 1, "exactly one durable watch.callback_rejected row was appended: {rows:?}");
        let row = &rows[0];
        assert_eq!(row.decision, "deny", "the skip is a deny decision");
        assert_eq!(row.actor_id, "watcher", "the durable row names the watch's owning applet as actor");
        assert_eq!(row.resource_id.as_deref(), Some("watch:open"), "the row names the watch");
        assert_eq!(row.metadata["reason"], json!("run_logs_cap"), "the row names the run_logs cap reason");
        assert_eq!(row.metadata["callback"], json!("onWatch"), "the row records the callback id");
    }

    // REOPEN the file-backed workspace (simulate restart): the EventSink is gone, but the
    // durable audit row is still queryable — the skip decision survived the process exit.
    let core = WorkspaceCore::open(&path, "ws-183-run").unwrap();
    let rows = durable_skip_rows(&core);
    assert_eq!(rows.len(), 1, "the durable skip decision survives a store reopen/restart: {rows:?}");
    assert_eq!(rows[0].actor_id, "watcher");
    assert_eq!(rows[0].resource_id.as_deref(), Some("watch:open"));
    assert_eq!(rows[0].metadata["reason"], json!("run_logs_cap"));
}

/// P1 (review 183): a `ui.dispatch_event` whose handler write ENTERS a watched result
/// admits its own dispatch record, then SKIPS the over-cap watcher callback — and the
/// skip is persisted into the DURABLE audit log, surviving the discarded batch AND a
/// reopen. `ui.dispatch_event` discards the `DeliveredBatch` (and the transient event is
/// in-memory only), so the durable row is what replay reproduces.
#[test]
fn ui_dispatch_over_cap_callback_skip_survives_in_durable_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws.forge");
    {
        let mut core = WorkspaceCore::open(&path, "ws-183-ui").unwrap();
        install(&mut core, "app", DEMO_TS);
        install(&mut core, "watcher", WATCH_TS);
        // Establish `app`'s render base (so ui.dispatch_event has a diff base/session) and
        // register the `done = false` watch.
        let base = run_with_title(&mut core, "app", "base");
        assert!(base.ok && base.payload["ok"] == Value::Bool(true), "app main must establish a render base");
        let watch_resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
        assert!(watch_resp.ok, "watch registration run must succeed: {:?}", watch_resp.error);
        assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

        pin_one_record_headroom(&mut core);
        core.events_mut().drain();

        // The dispatch's `bump` handler inserts a `done:false` task that ENTERS the watch;
        // the dispatch is admitted, its record lands (over cap), so the onWatch callback is
        // SKIPPED. The dispatch SUCCEEDS and discards its batch.
        let dispatch = core.handle(cmd(
            "ui.dispatch_event",
            Some("app"),
            json!({ "action_ref": "bump", "event_payload": {} }),
        ));
        assert!(dispatch.ok, "the near-cap ui.dispatch_event itself is admitted and succeeds: {dispatch:?}");
        let callback_wrote = core
            .store()
            .list_records("tasks")
            .unwrap()
            .into_iter()
            .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
        assert!(!callback_wrote, "the over-cap watcher callback commits NO `ctx.db` write");

        // The transient event is still emitted for live observability (review 179 P2
        // regression) — but it lives only in the in-memory sink.
        let events = core.events_mut().drain();
        assert!(
            events.iter().any(|e| e.kind == "db.watch.callback_rejected"
                && e.payload["watch_id"] == json!("watch:open")
                && e.payload["reason"] == json!("run_logs_cap")),
            "the skipped over-cap callback still emits the transient db.watch.callback_rejected event: {events:?}"
        );

        // The DURABLE row is present after the dispatch discarded its batch AND the event
        // buffer was drained — the decision is NOT lost with the transient batch/event.
        let rows = durable_skip_rows(&core);
        assert_eq!(rows.len(), 1, "exactly one durable watch.callback_rejected row was appended: {rows:?}");
        assert_eq!(rows[0].actor_id, "watcher", "the durable row names the owning applet");
        assert_eq!(rows[0].resource_id.as_deref(), Some("watch:open"));
        assert_eq!(rows[0].metadata["reason"], json!("run_logs_cap"));
    }

    // REOPEN: the in-memory event buffer is gone, but the durable decision survives.
    let core = WorkspaceCore::open(&path, "ws-183-ui").unwrap();
    let rows = durable_skip_rows(&core);
    assert_eq!(rows.len(), 1, "the durable skip decision survives a store reopen/restart: {rows:?}");
    assert_eq!(rows[0].actor_id, "watcher");
    assert_eq!(rows[0].resource_id.as_deref(), Some("watch:open"));
}

/// P1 (review 183): the time-travel `db.restore` path also persists the over-cap skip
/// durably. `db.restore` is a command (not an applet run), so pinning the cap to exactly
/// the current run_logs leaves NO headroom for the watcher callback re-entry: the restore
/// re-enters the watch, the callback is SKIPPED, and the decision lands as a durable
/// `watch.callback_rejected` row even though `cmd_db_restore` discards the batch.
#[test]
fn time_travel_restore_over_cap_callback_skip_persists_durably() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.sqlite");
    {
        let mut core = WorkspaceCore::open(&path, "ws-183-restore").unwrap();
        install(&mut core, "watcher", WATCH_TS);
        // Register the `done = false` watch (its own registration run records fine).
        let watch_resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
        assert!(watch_resp.ok, "watch registration run must succeed: {:?}", watch_resp.error);
        assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

        // Seed `tasks/t1` with history through the CRDT path (bypasses notifications): v1
        // is `done:false` (IN the watch), v2 flips it to `done:true` (LEAVES the watch).
        // Restoring to v1 re-enters the watch and would re-deliver the onWatch callback.
        let idx = IndexManager::new();
        core.store_mut()
            .apply_mutation_crdt(
                &Mutation::Insert {
                    collection: "tasks".into(),
                    id: Some("t1".into()),
                    fields: json!({ "title": "seed", "done": false })
                        .as_object()
                        .unwrap()
                        .clone(),
                    logical_at: Some(1),
                },
                &idx,
            )
            .unwrap();
        core.store_mut()
            .apply_mutation_crdt(
                &Mutation::Patch {
                    collection: "tasks".into(),
                    id: "t1".into(),
                    fields: json!({ "done": true }).as_object().unwrap().clone(),
                    logical_at: Some(2),
                },
                &idx,
            )
            .unwrap();

        // Zero run-log headroom: the watcher callback re-entry triggered by the restore
        // cannot be admitted (the restore command itself persists no run record, so it
        // commits).
        let cap = run_logs_bytes(&core);
        pin_run_logs_cap(&mut core, cap);

        // Restore t1 to v1 (`done:false`) through the LIVE command — this re-enters the
        // watch and SKIPS the over-cap callback. `cmd_db_restore` discards the returned
        // batch.
        let resp = core.handle(cmd(
            "db.restore",
            None,
            json!({ "collection": "tasks", "id": "t1", "to_version": 1, "restored_logical_at": 5 }),
        ));
        assert!(resp.ok, "db.restore should succeed (a downstream callback skip is not a producer failure): {:?}", resp.error);

        // The over-cap callback never ran.
        let callback_wrote = core
            .store()
            .list_records("tasks")
            .unwrap()
            .into_iter()
            .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
        assert!(!callback_wrote, "the over-cap watcher callback commits NO `ctx.db` write on the restore path");

        // The DURABLE decision is present after the restore command discarded its batch.
        let rows = durable_skip_rows(&core);
        assert_eq!(rows.len(), 1, "the time-travel restore path also persists exactly one durable skip row: {rows:?}");
        assert_eq!(rows[0].decision, "deny");
        assert_eq!(rows[0].actor_id, "watcher", "the durable row names the owning applet");
        assert_eq!(rows[0].resource_id.as_deref(), Some("watch:open"));
        assert_eq!(rows[0].metadata["reason"], json!("run_logs_cap"));
    }

    // REOPEN: the transient event/batch is gone, but the restore-path durable audit
    // decision survives exactly like the runtime.run and ui.dispatch_event cases.
    let core = WorkspaceCore::open(&path, "ws-183-restore").unwrap();
    let rows = durable_skip_rows(&core);
    assert_eq!(
        rows.len(),
        1,
        "the restore-path durable skip decision survives reopen: {rows:?}"
    );
    assert_eq!(rows[0].decision, "deny");
    assert_eq!(rows[0].actor_id, "watcher");
    assert_eq!(rows[0].resource_id.as_deref(), Some("watch:open"));
    assert_eq!(rows[0].metadata["reason"], json!("run_logs_cap"));
}

/// P2: `quota.status.approaching` enumerates EVERY budget at/above the approaching
/// threshold — the workspace total AND a per-applet collection budget AND a category cap
/// SIMULTANEOUSLY — not just the single strongest scope (review 177 regression).
#[test]
fn quota_status_approaching_enumerates_all_budgets_simultaneously() {
    let mut core = WorkspaceCore::in_memory("ws-appr").unwrap();
    install(&mut core, "app", DEMO_TS);

    // Seed some real records + run logs so the workspace/applet/category usages are all
    // non-trivial.
    for i in 0..4 {
        let resp = run_with_title(&mut core, "app", &format!("seed {i}"));
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "seed run must succeed");
    }

    let usage = core.quota_usage().unwrap();
    let ws = usage.workspace_total_bytes;
    let applet = usage.applet_bytes("tasks");
    let cat = usage.category_bytes(QuotaCategory::RunLogs);
    assert!(ws > 0 && applet > 0 && cat > 0, "all three buckets must carry bytes to be approachable");

    // Tighten the workspace, per-applet, and run_logs limits so EACH bucket sits at or
    // above the 80% threshold simultaneously. `limit = floor(usage / 0.8)` guarantees
    // `usage >= limit * 0.8` (each bucket is in the approaching band, not under it).
    let at_threshold = |bytes: u64| ((bytes as f64) / 0.8).floor() as u64;
    let policy = QuotaPolicy {
        workspace_limit: at_threshold(ws),
        per_applet_limit: at_threshold(applet),
        run_logs_cap: at_threshold(cat),
        approaching_threshold: 0.8,
        ..QuotaPolicy::DEFAULT
    };
    core.set_quota_policy(&policy).unwrap();

    let resp = core.handle(cmd("quota.status", None, json!({})));
    assert!(resp.ok, "quota.status must succeed: {:?}", resp.error);
    let approaching = resp.payload["approaching"].as_array().unwrap();
    let scopes: Vec<&str> = approaching
        .iter()
        .map(|w| w["scope"].as_str().unwrap())
        .collect();

    // ALL THREE distinct budgets are reported (the bug: only one would appear).
    assert!(scopes.contains(&"workspace"), "workspace budget must be listed: {scopes:?}");
    assert!(scopes.contains(&"applet:tasks"), "per-applet budget must be listed: {scopes:?}");
    assert!(scopes.contains(&"category:run_logs"), "run_logs category cap must be listed: {scopes:?}");

    // Each warning carries the projected/limit pair and the remedy suggestion (never a
    // deletion), and projected ≥ threshold * limit.
    for w in approaching {
        let projected = w["projected"].as_u64().unwrap();
        let limit = w["limit"].as_u64().unwrap();
        assert!(
            (projected as f64) >= (limit as f64) * 0.8,
            "an approaching warning sits at/above the threshold: {w}"
        );
        assert!(
            w["suggestion"].as_str().unwrap().contains("no data was deleted"),
            "the warning carries the compaction/cleanup/export suggestion: {w}"
        );
    }

    // The report is deterministic: two reads are byte-equal.
    let again = core.handle(cmd("quota.status", None, json!({})));
    assert_eq!(again.payload, resp.payload, "two quota.status reads are byte-equal");
}
