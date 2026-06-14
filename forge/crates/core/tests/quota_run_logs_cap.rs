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
use forge_storage::{Mutation, QuotaCategory, QuotaPolicy};
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

/// P1 (review 178): a `db.watch` callback whose re-entry cannot be admitted over the
/// tightened `run_logs_cap` is REJECTED PRE-FLIGHT — `commit_and_notify` propagates the
/// typed `ResourceLimitExceeded`, the callback never runs (no `ctx.db` write), and no
/// callback run record is written.
#[test]
fn run_logs_cap_rejects_watch_callback_preflight_with_no_side_effects() {
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
    // callback; the callback re-entry is REFUSED at the pre-flight admission gate, and the
    // whole notify propagates the typed rejection.
    let insert = Mutation::Insert {
        collection: "tasks".into(),
        id: Some("tasks/1".into()),
        fields: json!({ "title": "open task", "done": false }).as_object().unwrap().clone(),
        logical_at: Some(10),
    };
    // `tasks/1` is the triggering write the test issues directly — it commits as part of
    // commit_and_notify BEFORE the callback dispatch. The callback's OWN `ctx.db` write
    // ("callback wrote") must NOT land, and no callback run record must be persisted.
    let err = core.commit_and_notify(&insert).expect_err(
        "the watch callback's run admission must be rejected over the run_logs cap",
    );
    assert_eq!(err.code(), "ResourceLimitExceeded", "the callback rejection is a quota error: {err}");
    assert!(err.to_string().contains("no data was deleted"), "carries the cleanup/export suggestion: {err}");
    // NO unreplayable callback side effects: the callback's "callback wrote" record never
    // landed, and no callback run record was persisted.
    let callback_wrote = core
        .store()
        .list_records("tasks")
        .unwrap()
        .into_iter()
        .any(|r| r.fields.get("title").and_then(|v| v.as_str()) == Some("callback wrote"));
    assert!(!callback_wrote, "a rejected watch callback commits NO `ctx.db` write");
    assert_eq!(run_record_count(&core), runs_before, "a rejected watch callback persists NO run record");
    assert!(run_logs_bytes(&core) <= cap, "run_logs usage never exceeds the cap");
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
