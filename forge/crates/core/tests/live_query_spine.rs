//! End-to-end proof that the DL-16 live-query loop fires on a REAL applet
//! `ctx.db` write driven through the live run/dispatch spine — NOT the
//! manually-driven [`commit_and_notify`](forge_core::WorkspaceCore::commit_and_notify)
//! facade the substrate tests use.
//!
//! The gap these tests close (DWC1): before this wiring, a LIVE applet that did
//! `ctx.db.insert` during a `runtime.run`/`ui.dispatch_event` applied the write
//! directly through the bridge's CRDT path and fired NO watch notification. The
//! spine now drains the bridge's committed writes and drives their live-query
//! notifications, so a registered watch FIRES on a real applet mutation, is RECORDED
//! for replay, and re-enters the watch's `onWatch` callback. A callback that itself
//! mutates produces a SECOND notification at a strictly LATER version (the
//! non-reentrant next-turn loop, T047 (a)).

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, ActorId, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use serde_json::{json, Value};

/// A WATCHER applet: registers a live query over the open tasks in `main` and
/// exports an `onWatch` callback that re-renders from the notification's result ids.
const WATCHER_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.watch("watch:open", {
            from: "tasks",
            where: ["done", "=", false],
            orderBy: ["id", "asc"]
        });
        return { ok: true, value: { registered: true } };
    }

    export async function onWatch(ctx: any, n: any): Promise<any> {
        const ids = (n && n.result_ids) ? n.result_ids : [];
        await ctx.ui.render({ type: "Text", text: "open=" + ids.join(",") });
        ctx.log("onWatch v" + (n ? n.version : "?"));
        return { ok: true, value: { count: ids.length } };
    }
"#;

/// A WRITER applet: inserts a task through `ctx.db.insert` in `main`. Its run is
/// driven through the live spine, so the write fires the watcher's notification.
const WRITER_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const id = await ctx.db.insert("tasks", { title: input.title, done: false });
        return { ok: true, value: { id } };
    }
"#;

/// A WRITER that PATCHES a task done — making it LEAVE the open-tasks filter.
const PATCH_DONE_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.patch("tasks", input.id, { done: true });
        return { ok: true, value: {} };
    }
"#;

/// A WRITER that DELETES a task — also a LEAVE from the open-tasks filter.
const DELETE_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.delete("tasks", input.id);
        return { ok: true, value: {} };
    }
"#;

/// A WATCHER whose `onWatch` callback ITSELF inserts a new task (a second open
/// record) — the non-reentrant next-turn case: the callback's write commits as a
/// follow-up notification turn at a later version, never recursively.
const REACTIVE_WATCHER_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.db.watch("watch:open", {
            from: "tasks",
            where: ["done", "=", false],
            orderBy: ["id", "asc"]
        });
        return { ok: true, value: { registered: true } };
    }

    export async function onWatch(ctx: any, n: any): Promise<any> {
        const ids = (n && n.result_ids) ? n.result_ids : [];
        // React EXACTLY once (to the first delivery, when one task has arrived) so we
        // don't loop forever: insert a follow-up open task on that first notification.
        if (ids.length === 1) {
            await ctx.db.insert("tasks", { title: "spawned-by-callback", done: false });
        }
        await ctx.ui.render({ type: "Text", text: "open=" + ids.length });
        return { ok: true, value: { count: ids.length } };
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

fn install(core: &mut WorkspaceCore, id: &str, src: &str) {
    let resp = core.handle(cmd(
        "applet.install",
        Some(id),
        json!({ "manifest": manifest(), "sources": { "src/main.ts": src } }),
    ));
    assert!(resp.ok, "install {id} must succeed: {:?}", resp.error);
}

/// The canonical `db.watch.notification` event payloads emitted so far (the
/// observable notification stream the spine delivered).
fn notifications(core: &WorkspaceCore) -> Vec<Value> {
    core.events()
        .events_of_kind("db.watch.notification")
        .map(|e| e.payload.clone())
        .collect()
}

/// DWC1 (deliverables 1+2): a watch registered through the live spine FIRES on a
/// real applet `ctx.db.insert` driven through `runtime.run` — proving the bridge's
/// committed write routes through the notification path (it did NOT before this
/// wiring). The notification is emitted (and recorded) and re-enters the watcher's
/// `onWatch` callback through the engine.
#[test]
fn live_applet_ctx_db_insert_through_run_fires_a_registered_watch() {
    let mut core = WorkspaceCore::in_memory("ws-spine-fire").expect("workspace");
    install(&mut core, "watcher", WATCHER_TS);
    install(&mut core, "writer", WRITER_TS);

    // The watcher registers its live query in `main` → a workspace watch.
    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "watcher run: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);
    // No notification yet — nothing has been written.
    assert!(notifications(&core).is_empty(), "no write yet → no notification");

    // The WRITER applet inserts a task through ctx.db.insert in its own run. BEFORE
    // this wiring this fired no notification; now the spine drives it.
    let resp = core.handle(cmd(
        "runtime.run",
        Some("writer"),
        json!({ "input": { "title": "ship it" } }),
    ));
    assert!(resp.ok, "writer run: {:?}", resp.error);

    // The watched result now contains the inserted task — the write landed.
    assert_eq!(
        core.watch_result_ids("watch:open").unwrap().unwrap(),
        vec!["tasks/1".to_string()],
        "the writer's ctx.db.insert landed in the watched result"
    );

    // PROOF the watch FIRED on the real applet mutation: exactly one notification was
    // delivered, naming the watch + the real inserted id + the post-write result.
    let notes = notifications(&core);
    assert_eq!(notes.len(), 1, "the writer's insert fired exactly one notification");
    let n = &notes[0];
    assert_eq!(n["watch_id"], json!("watch:open"));
    assert_eq!(n["record_ids"], json!(["tasks/1"]));
    assert_eq!(n["result_ids"], json!(["tasks/1"]));
    assert_eq!(n["reason"], json!("insert"));

    // PROOF the callback was RE-ENTERED through the engine: the notification dispatch
    // re-rendered the watcher's view, emitted as a `ui.patch` for the watcher applet.
    let watcher_patch = core
        .events()
        .events_of_kind("ui.patch")
        .any(|e| e.applet_id.as_ref().map(|a| a.as_str()) == Some("watcher"));
    assert!(
        watcher_patch,
        "the onWatch callback re-rendered (proves it ran with the notification payload)"
    );
}

/// DWC1 (deliverable 3): a notification callback that MUTATES through `ctx.db`
/// produces a SECOND notification at a strictly LATER version — the non-reentrant
/// next-turn loop. The watcher's `onWatch` inserts a follow-up task on the first
/// delivery; that callback write commits as a SEPARATE turn whose version is greater
/// than the first, and re-enters the callback again (now with two open ids).
#[test]
fn watch_callback_mutation_delivers_a_second_notification_at_a_later_version() {
    let mut core = WorkspaceCore::in_memory("ws-spine-nextturn").expect("workspace");
    install(&mut core, "watcher", REACTIVE_WATCHER_TS);
    install(&mut core, "writer", WRITER_TS);

    let resp = core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    assert!(resp.ok, "watcher run: {:?}", resp.error);
    assert_eq!(core.active_watch_ids(), vec!["watch:open".to_string()]);

    // The writer inserts ONE task. That fires the first notification (version V); the
    // callback reacts by inserting a SECOND task, which fires a second notification
    // (version V+1) — the next-turn loop, never recursive.
    let resp = core.handle(cmd(
        "runtime.run",
        Some("writer"),
        json!({ "input": { "title": "first" } }),
    ));
    assert!(resp.ok, "writer run: {:?}", resp.error);

    // The callback's insert landed: the watched result now has TWO open tasks.
    let result = core.watch_result_ids("watch:open").unwrap().unwrap();
    assert_eq!(result.len(), 2, "the callback inserted a second open task: {result:?}");

    // At least TWO notifications were delivered (the write + the callback's next-turn
    // write), with strictly increasing versions — the callback's mutation was a
    // SEPARATE later turn, not a recursive flush inside the first delivery.
    let versions: Vec<u64> = notifications(&core)
        .iter()
        .filter_map(|n| n["version"].as_u64())
        .collect();
    assert!(
        versions.len() >= 2,
        "expected >=2 notifications (write + callback next-turn write), got versions {versions:?}"
    );
    assert!(
        versions[1] > versions[0],
        "the callback-mutation notification must carry a strictly LATER version (next-turn, not recursive): {versions:?}"
    );

    // The first notification carried one open id; the second (the callback's turn)
    // carried both — the next-turn delivery observed the callback's own write.
    let notes = notifications(&core);
    assert_eq!(notes[0]["result_ids"].as_array().unwrap().len(), 1);
    assert_eq!(
        notes[1]["result_ids"].as_array().unwrap().len(),
        2,
        "the second (callback next-turn) notification reflects the callback's own insert"
    );
}

/// A LIVE applet `ctx.db.patch`/`delete` that makes a watched record LEAVE the filter
/// must fire a notification through the spine. This is the case the post-write
/// snapshot cannot recover (the record is gone from the result after the write), so
/// the bridge captures the watch membership IMMEDIATELY BEFORE each write — the fix
/// the `before/after` enter/leave/changed filter depends on.
#[test]
fn live_patch_or_delete_that_leaves_the_watched_filter_notifies() {
    let mut core = WorkspaceCore::in_memory("ws-spine-leave").expect("workspace");
    install(&mut core, "watcher", WATCHER_TS);
    install(&mut core, "writer", WRITER_TS);
    install(&mut core, "patcher", PATCH_DONE_TS);
    install(&mut core, "deleter", DELETE_TS);

    core.handle(cmd("runtime.run", Some("watcher"), json!({ "input": {} })));
    // Insert an open task → notification #1 (it ENTERS).
    core.handle(cmd("runtime.run", Some("writer"), json!({ "input": { "title": "a" } })));
    let after_insert = notifications(&core).len();
    assert_eq!(after_insert, 1, "the insert ENTERS the open filter → one notification");

    // PATCH it done → it LEAVES the open filter → must notify (the record is no longer
    // in the post-write result, so this only fires with the pre-write snapshot).
    core.handle(cmd("runtime.run", Some("patcher"), json!({ "input": { "id": "tasks/1" } })));
    let after_patch = notifications(&core);
    assert_eq!(
        after_patch.len(),
        2,
        "the patch-done LEAVES the open filter → a notification fired"
    );
    assert_eq!(after_patch[1]["record_ids"], json!(["tasks/1"]));
    assert!(
        after_patch[1]["result_ids"].as_array().unwrap().is_empty(),
        "after the patch the watched result is empty (the task left)"
    );
    // The open-filter watch now sees nothing.
    assert!(core.watch_result_ids("watch:open").unwrap().unwrap().is_empty());

    // Insert another open task (notification #3, ENTER), then DELETE it (LEAVE → notify).
    core.handle(cmd("runtime.run", Some("writer"), json!({ "input": { "title": "b" } })));
    let before_delete = notifications(&core).len();
    core.handle(cmd("runtime.run", Some("deleter"), json!({ "input": { "id": "tasks/2" } })));
    let after_delete = notifications(&core);
    assert_eq!(
        after_delete.len(),
        before_delete + 1,
        "the delete LEAVES the open filter → a notification fired"
    );
    assert_eq!(after_delete.last().unwrap()["reason"], json!("delete"));
}
