//! End-to-end proof that the DL-16 live-query loop re-enters a REAL applet's watch
//! callback through the engine — not just the recorded-notification substrate.
//!
//! A TypeScript applet registers a `db.watch` (`ctx.db.watch`) in its `main` and
//! exports an `onWatch` callback. After a committed mutation makes the watched
//! collection dirty, [`commit_and_notify`](forge_core::WorkspaceCore::commit_and_notify)
//! computes the canonical notification, RECORDS it, and DISPATCHES it by re-entering
//! the `onWatch` handler over the same QuickJS containment / capability gate / record
//! path as a UI dispatch. The callback re-renders from the notification's
//! `result_ids` — proving the reactive loop is wired end to end (deliverables 1, 2,
//! 4 of DL-16 Phase 2).

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, ActorId, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use forge_storage::Mutation;
use serde_json::json;

/// An applet that REGISTERS a live query in `main` (via `ctx.db.watch`) and exports
/// an `onWatch` callback. The callback receives the canonical notification and
/// re-renders a Text listing the matched result ids — observable proof that the
/// callback ran with the real notification payload.
const WATCH_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.watch("watch:tasks-open", {
            from: "tasks",
            where: ["done", "=", false],
            orderBy: ["id", "asc"]
        });
        ctx.log("watch registered");
        return { ok: true, value: { registered: true } };
    }

    export async function onWatch(ctx: any, n: any): Promise<any> {
        // Re-render from the notification's result ids (no follow-up all() needed).
        const ids = (n && n.result_ids) ? n.result_ids : [];
        await ctx.ui.render({ type: "Text", text: "open=" + ids.length + ":" + ids.join(",") });
        ctx.log("onWatch v" + (n ? n.version : "?"));
        return { ok: true, value: { handled: true, count: ids.length } };
    }
"#;

fn manifest() -> serde_json::Value {
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
            "wall_ms": 3000,
            "fuel": 10000000,
            "memory_bytes": 67108864,
            "max_host_calls": 10000,
            "storage_bytes": 10485760,
            "log_bytes": 262144
        }
    })
}

fn owner() -> ActorContext {
    ActorContext { actor: ActorId::new("owner"), role: Role::Owner }
}

fn cmd(name: &str, applet_id: Option<&str>, payload: serde_json::Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor: owner(),
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: applet_id.map(AppletId::new),
        name: name.into(),
        payload,
    }
}

fn insert(id: &str, done: bool, at: i64) -> Mutation {
    Mutation::Insert {
        collection: "tasks".into(),
        id: Some(id.into()),
        fields: json!({ "title": id, "done": done }).as_object().unwrap().clone(),
        logical_at: Some(at),
    }
}

#[test]
fn real_applet_registers_a_watch_in_main_and_its_callback_is_re_entered_on_commit() {
    let mut core = WorkspaceCore::in_memory("ws-live-cb").expect("workspace");

    // Install the watching applet.
    let resp = core.handle(cmd(
        "applet.install",
        Some("watcher"),
        json!({ "manifest": manifest(), "sources": { "src/main.ts": WATCH_TS } }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);

    // Run `main` → it calls `ctx.db.watch`, which the facade folds into the workspace
    // registry as a live `watch:tasks-open` owned by `watcher` with the `onWatch`
    // callback.
    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "run must succeed: {:?}", resp.error);
    assert_eq!(
        core.active_watch_ids(),
        vec!["watch:tasks-open".to_string()],
        "ctx.db.watch in main registered the workspace watch"
    );

    // The watch's initial result is empty (no tasks yet).
    assert_eq!(
        core.watch_result_ids("watch:tasks-open").unwrap().unwrap(),
        Vec::<String>::new()
    );

    // Commit a mutation that enters the watched result → the facade computes the
    // notification AND re-enters the `onWatch` callback (real engine dispatch).
    let batch = core.commit_and_notify(&insert("tasks/1", false, 10)).expect("commit_and_notify");
    assert_eq!(batch.notifications.len(), 1, "one notification for the one watch");
    let n = &batch.notifications[0];
    assert_eq!(n.watch_id, "watch:tasks-open");
    assert_eq!(n.reason.as_str(), "insert");
    assert_eq!(n.record_ids, vec!["tasks/1"]);
    assert_eq!(n.result_ids, vec!["tasks/1"], "tasks/1 is open → in the result");
    // The notification was RECORDED (the replayable stream).
    assert_eq!(batch.recorded_calls.len(), 1);
    assert_eq!(batch.recorded_calls[0].method, "db.watch.notification");

    // Proof the callback was RE-ENTERED through the engine: the batch records the
    // callback's run id, and that saved run's trace carries the `db.watch.notification`
    // envelope (the recorded delivery) AND a `ui.render` (the callback's re-render).
    assert_eq!(batch.callback_runs.len(), 1, "the watch's onWatch callback was re-entered once");
    let callback_run = core
        .store()
        .load_run(batch.callback_runs[0].as_str())
        .expect("load callback run")
        .expect("callback run was saved");
    assert!(
        callback_run.calls.iter().any(|c| c.method == "db.watch.notification"),
        "the callback run records the delivered notification envelope"
    );
    assert!(
        callback_run.calls.iter().any(|c| c.method == "ui.render"),
        "the callback re-rendered from the notification (proves it ran with the payload)"
    );

    // A second commit that does NOT enter the result (a done task) still dirties but
    // delivers nothing to the open-filter watch.
    let batch2 = core.commit_and_notify(&insert("tasks/2", true, 20)).expect("commit 2");
    assert!(batch2.notifications.is_empty(), "a done task is outside the open filter → no notify");

    // db.unwatch (command path) stops later notifications.
    let resp = core.handle(cmd(
        "db.unwatch",
        Some("watcher"),
        json!({ "watch_id": "watch:tasks-open" }),
    ));
    assert!(resp.ok, "unwatch must succeed: {:?}", resp.error);
    assert!(core.active_watch_ids().is_empty(), "watch cancelled");
    let batch3 = core.commit_and_notify(&insert("tasks/3", false, 30)).expect("commit 3");
    assert!(batch3.notifications.is_empty(), "no watch → no notification after unwatch");
}

#[test]
fn db_watch_command_gates_on_db_read_and_returns_initial_result() {
    let mut core = WorkspaceCore::in_memory("ws-cmd").expect("workspace");
    let resp = core.handle(cmd(
        "applet.install",
        Some("watcher"),
        json!({ "manifest": manifest(), "sources": { "src/main.ts": WATCH_TS } }),
    ));
    assert!(resp.ok, "install: {:?}", resp.error);

    // A Viewer (read-capable) may register a watch via the command path.
    let resp = core.handle(CoreCommand {
        request_id: RequestId::new("r"),
        actor: ActorContext { actor: ActorId::new("v"), role: Role::Viewer },
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: Some(AppletId::new("watcher")),
        name: "db.watch".into(),
        payload: json!({
            "watch_id": "watch:open",
            "query": { "from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"] },
            "callback": "onWatch"
        }),
    });
    assert!(resp.ok, "Viewer (db.read-capable) may register a watch: {:?}", resp.error);
    assert_eq!(resp.payload["watch_id"], json!("watch:open"));
    assert_eq!(resp.payload["active"], json!(true));

    // A Runner (execution-only, NOT a data reader) is denied at the command-level gate.
    let resp = core.handle(CoreCommand {
        request_id: RequestId::new("r"),
        actor: ActorContext { actor: ActorId::new("runner"), role: Role::Runner },
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: Some(AppletId::new("watcher")),
        name: "db.watch".into(),
        payload: json!({ "watch_id": "watch:x", "query": { "from": "tasks" } }),
    });
    assert!(!resp.ok, "a Runner lacks db.read → cannot register a watch");
    assert_eq!(resp.error.as_ref().unwrap().code(), "PermissionDenied");

    // An aggregate watch is rejected (review 129 #2): no row result_ids.
    let resp = core.handle(cmd(
        "db.watch",
        Some("watcher"),
        json!({ "watch_id": "watch:count", "query": { "from": "tasks", "aggregate": { "count": true } } }),
    ));
    assert!(!resp.ok, "an aggregate watch has no row result_ids → rejected");
    assert_eq!(resp.error.as_ref().unwrap().code(), "QueryError");
}
