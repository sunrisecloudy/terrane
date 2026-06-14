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

/// A watching applet whose `onWatch` callback MUTATES the watched collection via
/// `ctx.db.insert` (review 132 #1). To stay non-reentrant AND terminate, it inserts
/// exactly one follow-up open task only while the result set is still a singleton —
/// so the first notification's callback inserts a second open task (a NEXT-turn
/// write), and the second notification (now 2 results) inserts nothing.
const MUTATING_WATCH_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.watch("watch:tasks-open", {
            from: "tasks",
            where: ["done", "=", false],
            orderBy: ["id", "asc"]
        });
        return { ok: true, value: { registered: true } };
    }

    export async function onWatch(ctx: any, n: any): Promise<any> {
        const ids = (n && n.result_ids) ? n.result_ids : [];
        // Insert a follow-up open task ONLY while the result is still a singleton, so
        // the callback's write happens exactly once (the NEXT turn sees 2 results and
        // inserts nothing) — proving the mutation is queued, not recursively flushed.
        if (ids.length < 2) {
            await ctx.db.insert("tasks", { title: "mirror", done: false });
        }
        return { ok: true, value: { handled: true, count: ids.length } };
    }
"#;

#[test]
fn watch_callback_db_insert_drives_a_next_turn_notification_with_a_later_version() {
    // review 132 #1: a REAL `onWatch` handler that calls `ctx.db.insert` on the
    // watched collection must produce the next event-loop turn's notification (a
    // strictly LATER version), never a recursive flush inside the first batch.
    let mut core = WorkspaceCore::in_memory("ws-reentrant").expect("workspace");
    let resp = core.handle(cmd(
        "applet.install",
        Some("watcher"),
        json!({ "manifest": manifest(), "sources": { "src/main.ts": MUTATING_WATCH_TS } }),
    ));
    assert!(resp.ok, "install: {:?}", resp.error);
    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "run: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:tasks-open".to_string()]);

    // Turn 1: insert tasks/1 (open). The facade delivers v1, re-enters `onWatch`,
    // whose `ctx.db.insert` adds a SECOND open task — captured and driven as the NEXT
    // turn (v2), never recursively inside this batch.
    let batch = core.commit_and_notify(&insert("tasks/1", false, 10)).expect("commit");

    // TWO notifications were delivered across the turn loop: v1 for the original
    // insert, v2 for the callback's queued write — strictly increasing versions.
    let versions: Vec<u64> = batch.notifications.iter().map(|n| n.version).collect();
    assert_eq!(versions.len(), 2, "two notifications: original + callback's next turn");
    assert!(versions[1] > versions[0], "the callback's notification has a LATER version");

    // The first notification is the original insert; the second is the callback's
    // queued write entering the result (now two open tasks).
    assert_eq!(batch.notifications[0].record_ids, vec!["tasks/1"]);
    assert_eq!(batch.notifications[0].result_ids, vec!["tasks/1"]);
    assert_eq!(
        batch.notifications[1].result_ids,
        vec!["tasks/1", "tasks/2"],
        "the callback's inserted task entered the watched result on the next turn"
    );

    // The callback re-entered TWICE (once per delivered notification); the SECOND
    // re-entry inserted nothing (result length == 2), so the loop terminated — proof
    // the mutation queued for the next turn rather than recursing without bound.
    assert_eq!(batch.callback_runs.len(), 2, "onWatch re-entered once per notification");
    assert!(batch.queued_mutations.is_empty(), "the turn loop drained to quiescence");

    // Both notifications RECORD + REPLAY byte-identically.
    assert_eq!(batch.recorded_calls.len(), 2);
    assert!(batch.recorded_calls.iter().all(|c| c.method == "db.watch.notification"));

    // The store actually holds the callback's inserted task (it committed once).
    assert_eq!(core.store().list_records("tasks").unwrap().len(), 2);
}

#[test]
fn db_unwatch_is_owner_scoped_one_applet_cannot_cancel_another_applets_watch() {
    // review 132 #2: watch ids are applet-visible strings, but one applet must NOT be
    // able to cancel another applet's subscription by naming its id. `db.unwatch` is
    // owner-scoped — applet B's unwatch of applet A's watch is a no-op and A keeps
    // receiving notifications.
    let mut core = WorkspaceCore::in_memory("ws-owner").expect("workspace");
    for app in ["app-a", "app-b"] {
        let resp = core.handle(cmd(
            "applet.install",
            Some(app),
            json!({ "manifest": manifest(), "sources": { "src/main.ts": WATCH_TS } }),
        ));
        assert!(resp.ok, "install {app}: {:?}", resp.error);
    }

    // app-a registers the watch (owns "watch:tasks-open").
    let resp = core.handle(cmd("runtime.run", Some("app-a"), json!({ "input": {} })));
    assert!(resp.ok, "app-a run: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:tasks-open".to_string()]);

    // app-b attempts to cancel app-a's watch by its (guessable) id → no-op.
    let resp = core.handle(cmd(
        "db.unwatch",
        Some("app-b"),
        json!({ "watch_id": "watch:tasks-open" }),
    ));
    assert!(resp.ok, "unwatch command succeeds (idempotent): {:?}", resp.error);
    assert_eq!(
        resp.payload["was_active"],
        json!(false),
        "app-b does not own the watch → its unwatch is a non-destructive no-op"
    );
    assert_eq!(
        core.active_watch_ids(),
        vec!["watch:tasks-open".to_string()],
        "app-a's watch survives app-b's unwatch attempt"
    );

    // app-a still receives its notification.
    let batch = core.commit_and_notify(&insert("tasks/1", false, 10)).expect("commit");
    assert_eq!(batch.notifications.len(), 1, "app-a's watch is still live → it is notified");
    assert_eq!(batch.notifications[0].watch_id, "watch:tasks-open");

    // The OWNER (app-a) can cancel its own watch.
    let resp = core.handle(cmd(
        "db.unwatch",
        Some("app-a"),
        json!({ "watch_id": "watch:tasks-open" }),
    ));
    assert!(resp.ok, "owner unwatch: {:?}", resp.error);
    assert_eq!(resp.payload["was_active"], json!(true), "the owner cancelled its live watch");
    assert!(core.active_watch_ids().is_empty(), "owner's unwatch cancelled the watch");
}
