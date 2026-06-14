//! DL-22 review 177 LIVE-WIRING proofs that the `run_logs` cap is ENFORCED on run
//! persistence (not report-only), and that `quota.status.approaching` enumerates EVERY
//! budget at/above the threshold.
//!
//! P1 — the run-log cap is enforced on EVERY run-persistence path. Run records
//! (`runs.record_json`) are part of the `run_logs` category (`spec/quotas.md` §1). DL-22
//! caps run logs and a privileged `quota.set` can tighten `run_logs_cap`, but the run
//! PERSISTENCE path had no gate, so the cap was report-only: a tightened cap was
//! reported by `quota.status` while later runs kept appending run records beyond it.
//! Now `Store::save_run_with_quota_tx` gates the three run-persistence paths
//! (`runtime.run`, `ui.dispatch_event`, and the `db.watch` callback): once
//! `run_logs_cap` sits below the next run record's bytes, each is REJECTED with a typed
//! `ResourceLimitExceeded` + the compaction/cleanup/export suggestion, the transaction
//! rolls back (reject-not-delete), and the `run_logs` usage NEVER exceeds the cap.
//!
//! P2 — `quota.status.approaching` lists ALL budgets at/above the threshold. With the
//! workspace total AND a per-applet budget AND a category cap simultaneously above the
//! approaching threshold, the report enumerates ALL THREE (it no longer proxies through
//! a single-scope `decide_quota`, which masked simultaneous warnings).

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};
use forge_storage::{Mutation, QuotaCategory, QuotaPolicy};
use serde_json::{json, Value};

/// A demo applet whose `main` inserts a `tasks` record (so `runtime.run` persists a
/// run record AND grows records) and whose `bump` UI handler re-renders (so a
/// `ui.dispatch_event` persists a run record too).
const DEMO_TS: &str = r#"
    export async function main(ctx, input) {
        const title = (input && input.title) ? input.title : "untitled";
        await ctx.db.insert("tasks", { title: title, done: false });
        ctx.ui.render({ type: "Text", text: title });
        return { ok: true, value: { ok: true } };
    }
    export const handlers = {
        "bump": async (ctx, _event) => {
            ctx.ui.render({ type: "Text", text: "bumped" });
            return { ok: true, value: { bumped: true } };
        },
    };
"#;

/// A watching applet: `main` registers a `db.watch` over open tasks and `onWatch`
/// re-renders. Its callback re-entry persists a run record (the third path).
const WATCH_TS: &str = r#"
    export async function main(ctx, input) {
        await ctx.db.watch("watch:open", { from: "tasks", where: ["done", "=", false], orderBy: ["id", "asc"] });
        return { ok: true, value: { registered: true } };
    }
    export async function onWatch(ctx, n) {
        const ids = (n && n.result_ids) ? n.result_ids : [];
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

/// Persist the current effective policy with `run_logs_cap` pinned to `cap` (the
/// trusted seam — never the write being checked).
fn pin_run_logs_cap(core: &mut WorkspaceCore, cap: u64) {
    let mut policy = core.quota_policy().unwrap();
    policy.run_logs_cap = cap;
    core.set_quota_policy(&policy).unwrap();
}

/// The typed-error shape a run-persistence REJECTION surfaces on a `runtime.run` /
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

/// P1: after tightening `run_logs_cap` below the next run record's size, a
/// `runtime.run` and a `ui.dispatch_event` that would persist a run record are each
/// REJECTED with a typed `ResourceLimitExceeded` + the suggestion, and the persisted
/// `run_logs` usage never exceeds the cap.
#[test]
fn run_logs_cap_is_enforced_on_runtime_run_and_ui_dispatch() {
    let mut core = WorkspaceCore::in_memory("ws-rl").unwrap();
    install(&mut core, "app", DEMO_TS);

    // Seed a few runs so run_logs is non-trivial, and establish a diff base + handler
    // session for the UI dispatch path.
    for i in 0..3 {
        let resp = run_with_title(&mut core, "app", &format!("seed {i}"));
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "seed run must succeed");
    }

    // Pin the run_logs cap to EXACTLY the current run_logs bytes: zero headroom, so the
    // next run record overflows the cap. (Other budgets stay roomy — this isolates the
    // run_logs cap as the binding limit.)
    let cap = run_logs_bytes(&core);
    pin_run_logs_cap(&mut core, cap);

    // runtime.run: the run executed in-memory but its record can't be persisted under
    // the cap → REJECTED. The record never lands; run_logs stays at the cap.
    core.events_mut().drain();
    let resp = run_with_title(&mut core, "app", "this run record does not fit");
    let (code, detail) = rejection(&resp).expect("runtime.run must be rejected over the run_logs cap");
    assert_eq!(code, "ResourceLimitExceeded", "runtime.run rejection is a quota error: {resp:?}");
    assert!(detail.contains("no data was deleted"), "carries the cleanup/export suggestion: {detail}");
    assert!(detail.contains("run_logs") || detail.contains("category"), "names the run_logs budget: {detail}");
    assert!(run_logs_bytes(&core) <= cap, "run_logs usage never exceeds the tightened cap");

    // ui.dispatch_event: a re-render handler whose run record would also overflow the
    // cap is rejected the same way (the dispatch save routes through the same gate).
    // First render a base tree so the applet has a UI session, then dispatch.
    // (Re-pin in case the base render changed run_logs — keep zero headroom.)
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("app"),
        json!({ "action_ref": "bump", "event_payload": {} }),
    ));
    let (dcode, ddetail) = rejection(&dispatch).expect("ui.dispatch_event must be rejected over the run_logs cap");
    assert_eq!(dcode, "ResourceLimitExceeded", "ui.dispatch_event rejection is a quota error: {dispatch:?}");
    assert!(ddetail.contains("no data was deleted"), "carries the cleanup/export suggestion: {ddetail}");
    assert!(run_logs_bytes(&core) <= cap, "run_logs usage still never exceeds the cap after the dispatch");
}

/// P1: a `db.watch` callback whose re-entry would persist a run record over the
/// tightened `run_logs_cap` is BLOCKED — `commit_and_notify` propagates the typed
/// `ResourceLimitExceeded`, the callback run never lands, and run_logs stays at the cap.
#[test]
fn run_logs_cap_is_enforced_on_watch_callback_persistence() {
    let mut core = WorkspaceCore::in_memory("ws-rl-watch").unwrap();
    install(&mut core, "watcher", WATCH_TS);

    // Register the watch (its own run record persists fine under the default cap).
    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "watch registration run must succeed: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

    // Pin run_logs to zero headroom so the NEXT run record (the callback re-entry) can't
    // be persisted.
    let cap = run_logs_bytes(&core);
    pin_run_logs_cap(&mut core, cap);

    // A committed mutation that enters the watched result dispatches the onWatch
    // callback, which would persist a callback run record → the persistence is gated and
    // the whole notify propagates the typed rejection.
    let insert = Mutation::Insert {
        collection: "tasks".into(),
        id: Some("tasks/1".into()),
        fields: json!({ "title": "open task", "done": false }).as_object().unwrap().clone(),
        logical_at: Some(10),
    };
    let err = core.commit_and_notify(&insert).expect_err(
        "the watch callback's run-record persistence must be rejected over the run_logs cap",
    );
    assert_eq!(err.code(), "ResourceLimitExceeded", "the callback rejection is a quota error: {err}");
    assert!(err.to_string().contains("no data was deleted"), "carries the cleanup/export suggestion: {err}");
    assert!(run_logs_bytes(&core) <= cap, "run_logs usage never exceeds the tightened cap");
}

/// P2: `quota.status.approaching` enumerates EVERY budget at/above the approaching
/// threshold — the workspace total AND a per-applet collection budget AND a category cap
/// SIMULTANEOUSLY — not just the single strongest scope (the prior `decide_quota` proxy
/// masked the others).
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
