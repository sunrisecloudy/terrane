//! Applet lifecycle integration tests (CR-7 / `forge/spec/applet-lifecycle.md`,
//! `forge/fixtures/lifecycle/*`).
//!
//! These drive the lifecycle commands through the SAME facade a shell uses
//! (`WorkspaceCore::handle`) — install → enable/suspend/uninstall, the suspended-
//! dispatch gate on BOTH `runtime.run` and `ui.dispatch_event`, idempotency, and
//! the uninstall retention policies. Every transition is asserted through the
//! command registry, not a private seam, so the registry wiring is load-bearing.

use forge_core::{AppletLifecycle, WorkspaceCore};
use forge_domain::{ActorContext, ActorId, AppletId, CoreCommand, RequestId, Role, WorkspaceId};

/// A small interactive TS applet: `main` renders a Button bound to `todo.add`;
/// the `todo.add` handler re-renders the Button with the label "Task added".
/// Enough to exercise `runtime.run` (initial render) + `ui.dispatch_event` (the
/// handler) through the lifecycle gate.
const TODO_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        await ctx.ui.render({ type: "Button", testId: "add-task", label: "Add task", onTap: "todo.add" });
        return { ok: true, value: null };
    }
    export const handlers = {
        "todo.add": async (ctx: any, event: any): Promise<any> => {
            await ctx.ui.render({ type: "Button", testId: "add-task", label: "Task added", onTap: "todo.add" });
            return { ok: true, value: null };
        }
    };
"#;

/// A SECOND version of the todo applet, distinct from [`TODO_TS`] by a `v2` literal
/// so its transpiled JS — and therefore its `code_hash` — differs. Used to drive a
/// real `applet.upgrade` (a new version, not the idempotent same-code reinstall).
const V2_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const v = "v2";
        await ctx.ui.render({ type: "Button", testId: "add-task", label: "Add task", onTap: "todo.add" });
        return { ok: true, value: v };
    }
    export const handlers = {
        "todo.add": async (ctx: any, event: any): Promise<any> => {
            await ctx.ui.render({ type: "Button", testId: "add-task", label: "Task added", onTap: "todo.add" });
            return { ok: true, value: null };
        }
    };
"#;

/// Manifest granting db write to `tasks` (so uninstall purge has owned records to
/// tombstone) + ui.
fn todo_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "db": { "read": ["tasks"], "write": ["tasks"] },
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
    ActorContext::owner("alice")
}

fn cmd(name: &str, applet_id: Option<&str>, payload: serde_json::Value) -> CoreCommand {
    cmd_as(owner(), name, applet_id, payload)
}

fn cmd_as(
    actor: ActorContext,
    name: &str,
    applet_id: Option<&str>,
    payload: serde_json::Value,
) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor,
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: applet_id.map(AppletId::new),
        name: name.into(),
        payload,
    }
}

fn actor(role: Role) -> ActorContext {
    ActorContext { actor: ActorId::new(format!("{role:?}").to_lowercase()), role }
}

/// Install the todo applet (success), returning the install response payload.
fn install(core: &mut WorkspaceCore, manifest: serde_json::Value) -> serde_json::Value {
    let resp = core.handle(cmd(
        "applet.install",
        Some("applet.todo"),
        serde_json::json!({ "manifest": manifest, "sources": { "src/main.ts": TODO_TS } }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);
    resp.payload
}

// ---------------------------------------------------------------------------
// install_creates_enabled_v1
// ---------------------------------------------------------------------------

#[test]
fn install_creates_enabled_v1() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let payload = install(&mut core, todo_manifest());

    assert_eq!(payload["install_generation"], serde_json::json!(1), "first install is generation 1");
    assert_eq!(payload["version"], serde_json::json!(1), "first install is version 1");
    assert_eq!(payload["lifecycle"], serde_json::json!("enabled"), "fresh install is enabled");
    // Durable state: an active record exists and the trusted lifecycle is Active.
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Active);

    let installed = core.events().events_of_kind("applet.installed").count();
    assert_eq!(installed, 1, "an applet.installed audit event is emitted");
    let ev = core.events().events_of_kind("applet.installed").next().unwrap();
    assert_eq!(ev.payload["state_after"], serde_json::json!("enabled"));
    assert_eq!(ev.payload["install_generation"], serde_json::json!(1));
}

// ---------------------------------------------------------------------------
// enable_then_run_dispatches_event: suspended applet, enable, run, dispatch
// ---------------------------------------------------------------------------

#[test]
fn enable_then_run_dispatches_event() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Start suspended (admin-paused).
    core.set_applet_lifecycle("applet.todo", AppletLifecycle::Suspended).unwrap();

    // enable: suspended -> enabled.
    let enabled = core.handle(cmd("applet.enable", Some("applet.todo"), serde_json::json!({})));
    assert!(enabled.ok, "enable must succeed: {:?}", enabled.error);
    assert_eq!(enabled.payload["state"], serde_json::json!("enabled"));
    assert_eq!(enabled.payload["changed"], serde_json::json!(true));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Active);

    // run now dispatches (the initial render).
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": { "mode": "boot" } })));
    assert!(run.ok, "run on an enabled applet must succeed: {:?}", run.error);
    assert_eq!(run.payload["ok"], serde_json::json!(true));

    // dispatch the todo.add event → the handler re-renders with "Task added".
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("applet.todo"),
        serde_json::json!({ "action_ref": "todo.add", "event_payload": {} }),
    ));
    assert!(dispatch.ok, "dispatch on an enabled applet must succeed: {:?}", dispatch.error);
    let patches = dispatch.payload["patches"].to_string();
    assert!(patches.contains("Task added"), "handler re-rendered the label: {patches}");

    let enabled_events = core.events().events_of_kind("applet.enabled").count();
    assert_eq!(enabled_events, 1, "a real resume emits applet.enabled");
}

// ---------------------------------------------------------------------------
// suspend_rejects_dispatch: suspend, then dispatch is rejected before handler
// ---------------------------------------------------------------------------

#[test]
fn suspend_rejects_dispatch_before_handler() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Seed a last-known tree by running once while enabled.
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(run.ok);

    // suspend: enabled -> suspended.
    let suspend = core.handle(cmd(
        "applet.suspend",
        Some("applet.todo"),
        serde_json::json!({ "reason": "owner-paused" }),
    ));
    assert!(suspend.ok, "suspend must succeed: {:?}", suspend.error);
    assert_eq!(suspend.payload["state"], serde_json::json!("suspended"));
    assert_eq!(suspend.payload["changed"], serde_json::json!(true));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);

    // The applet's last-known tree before dispatch (the diff base, unchanged by a
    // rejected dispatch).
    let tree_before = core.store().kv_get("__forge/meta", "ui_tree/applet.todo").unwrap();

    // dispatch is rejected BEFORE any handler runs.
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("applet.todo"),
        serde_json::json!({ "action_ref": "todo.add", "event_payload": {} }),
    ));
    assert!(!dispatch.ok, "dispatch on a suspended applet must be rejected");
    let err = dispatch.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("ui.applet_not_dispatchable"),
        "carries the ui.applet_not_dispatchable marker: {err}"
    );
    assert!(err.to_string().contains("suspended"), "names the suspended state: {err}");

    // No state change: the lifecycle is still suspended and the diff base is unchanged.
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);
    let tree_after = core.store().kv_get("__forge/meta", "ui_tree/applet.todo").unwrap();
    assert_eq!(tree_before, tree_after, "a rejected dispatch leaves the UI tree unchanged");

    // The rejection emitted the spec-canonical `ui.dispatch_event.rejected` audit
    // (the `suspend_rejects_dispatch` vector) with dispatch_attempted=false. The
    // rejection code is carried under BOTH the spec-pinned `error_code` field (the
    // run path's `runtime.run.rejected` field) AND the renderer-facing `code`.
    let rejected = core
        .events()
        .events_of_kind("ui.dispatch_event.rejected")
        .next()
        .expect("a ui.dispatch_event.rejected event is emitted");
    assert_eq!(rejected.payload["dispatch_attempted"], serde_json::json!(false));
    assert_eq!(rejected.payload["error_code"], serde_json::json!("ui.applet_not_dispatchable"));
    assert_eq!(rejected.payload["code"], serde_json::json!("ui.applet_not_dispatchable"));
}

// ---------------------------------------------------------------------------
// suspend also gates runtime.run (T036 ties the gate to BOTH run + dispatch)
// ---------------------------------------------------------------------------

#[test]
fn suspend_rejects_runtime_run_before_user_code() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    core.handle(cmd("applet.suspend", Some("applet.todo"), serde_json::json!({})));

    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "runtime.run on a suspended applet must be rejected");
    let err = run.error.unwrap();
    assert!(
        err.to_string().contains("lifecycle.applet_suspended"),
        "carries the lifecycle.applet_suspended marker: {err}"
    );
    assert!(err.to_string().contains("suspended"));

    // No run was recorded, no run.started/run.completed emitted, only the rejection.
    assert_eq!(core.events().events_of_kind("run.started").count(), 0, "no user code started");
    assert_eq!(core.events().events_of_kind("run.completed").count(), 0);
    let rejected = core
        .events()
        .events_of_kind("runtime.run.rejected")
        .next()
        .expect("a runtime.run.rejected event is emitted");
    assert_eq!(rejected.payload["error_code"], serde_json::json!("lifecycle.applet_suspended"));
}

// ---------------------------------------------------------------------------
// reenable_resumes_dispatch
// ---------------------------------------------------------------------------

#[test]
fn reenable_resumes_dispatch() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Run once to seed a diff base, then suspend.
    core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    core.handle(cmd("applet.suspend", Some("applet.todo"), serde_json::json!({})));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);

    // re-enable, then dispatch resumes.
    let enabled = core.handle(cmd("applet.enable", Some("applet.todo"), serde_json::json!({})));
    assert!(enabled.ok);
    assert_eq!(enabled.payload["changed"], serde_json::json!(true));
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("applet.todo"),
        serde_json::json!({ "action_ref": "todo.add", "event_payload": {} }),
    ));
    assert!(dispatch.ok, "dispatch must resume after re-enable: {:?}", dispatch.error);
    assert!(dispatch.payload["patches"].to_string().contains("Task added"));
}

// ---------------------------------------------------------------------------
// enable is idempotent on an already-enabled applet
// ---------------------------------------------------------------------------

#[test]
fn enable_already_enabled_is_idempotent() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Already enabled (fresh install).
    let enabled = core.handle(cmd("applet.enable", Some("applet.todo"), serde_json::json!({})));
    assert!(enabled.ok);
    assert_eq!(enabled.payload["state"], serde_json::json!("enabled"));
    assert_eq!(enabled.payload["changed"], serde_json::json!(false));
    assert_eq!(enabled.payload["idempotent"], serde_json::json!(true));
    assert_eq!(core.events().events_of_kind("applet.enable.noop").count(), 1);
    // No spurious applet.enabled (state-change) event.
    assert_eq!(core.events().events_of_kind("applet.enabled").count(), 0);
}

// ---------------------------------------------------------------------------
// suspend_already_suspended_idempotent
// ---------------------------------------------------------------------------

#[test]
fn suspend_already_suspended_is_idempotent() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    core.set_applet_lifecycle("applet.todo", AppletLifecycle::Suspended).unwrap();

    let suspend = core.handle(cmd(
        "applet.suspend",
        Some("applet.todo"),
        serde_json::json!({ "reason": "owner-paused" }),
    ));
    assert!(suspend.ok);
    assert_eq!(suspend.payload["state"], serde_json::json!("suspended"));
    assert_eq!(suspend.payload["changed"], serde_json::json!(false));
    assert_eq!(suspend.payload["idempotent"], serde_json::json!(true));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);
    // The noop variant is emitted, not a fresh applet.suspended.
    assert_eq!(core.events().events_of_kind("applet.suspend.noop").count(), 1);
    assert_eq!(core.events().events_of_kind("applet.suspended").count(), 0);
}

// ---------------------------------------------------------------------------
// uninstall_keep_data_retains_records
// ---------------------------------------------------------------------------

#[test]
fn uninstall_keep_data_retains_records() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Seed a record the applet owns (collection `tasks` is in db.write).
    seed_task(&mut core, "tasks/1", "keep me");

    let uninstall = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "keep_data" }),
    ));
    assert!(uninstall.ok, "uninstall keep_data must succeed: {:?}", uninstall.error);
    assert_eq!(uninstall.payload["state"], serde_json::json!("uninstalled"));
    assert_eq!(uninstall.payload["retention"]["policy"], serde_json::json!("keep_data"));
    assert_eq!(uninstall.payload["retention"]["records_retained"], serde_json::json!(1));
    assert_eq!(uninstall.payload["retention"]["records_tombstoned"], serde_json::json!(0));

    // The active applet is gone, but the record survives undeleted.
    let rec = core.store().get_record("tasks", "tasks/1").unwrap().expect("record retained");
    assert!(!rec.deleted, "keep_data leaves the record live");
    assert_eq!(rec.fields["title"], serde_json::json!("keep me"));

    assert_eq!(core.events().events_of_kind("applet.uninstalled").count(), 1);
}

// ---------------------------------------------------------------------------
// uninstall_purge_data_tombstones_records
// ---------------------------------------------------------------------------

#[test]
fn uninstall_purge_data_tombstones_records() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    seed_task(&mut core, "tasks/1", "purge me");

    let uninstall = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "purge_data" }),
    ));
    assert!(uninstall.ok, "uninstall purge_data must succeed: {:?}", uninstall.error);
    assert_eq!(uninstall.payload["retention"]["policy"], serde_json::json!("purge_data"));
    assert_eq!(uninstall.payload["retention"]["records_retained"], serde_json::json!(0));
    assert_eq!(uninstall.payload["retention"]["records_tombstoned"], serde_json::json!(1));

    // The record is tombstoned (soft-deleted) with the purge reason.
    let rec = core.store().get_record("tasks", "tasks/1").unwrap().expect("tombstone row retained");
    assert!(rec.deleted, "purge_data tombstones the record");
    assert_eq!(
        rec.extensions["tombstone_reason"],
        serde_json::json!("applet.uninstall:purge_data"),
        "the tombstone carries the uninstall purge reason"
    );
}

// ---------------------------------------------------------------------------
// uninstall requires a retention policy (mandatory)
// ---------------------------------------------------------------------------

#[test]
fn uninstall_requires_a_retention_policy() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());

    let missing = core.handle(cmd("applet.uninstall", Some("applet.todo"), serde_json::json!({})));
    assert!(!missing.ok, "a retention policy is mandatory");
    assert!(missing.error.unwrap().to_string().contains("retention_policy"));
    // The applet is still installed (fail-closed).
    let still = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(still.ok, "a rejected uninstall leaves the applet installed + runnable");

    let bad = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "shred_everything" }),
    ));
    assert!(!bad.ok, "an unknown retention policy is rejected");
}

// ---------------------------------------------------------------------------
// run_uninstalled_rejected: an illegal transition is a typed rejection, no panic
// ---------------------------------------------------------------------------

#[test]
fn run_uninstalled_is_typed_rejection_not_panic() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "keep_data" }),
    ));

    // runtime.run on the uninstalled applet is a typed lifecycle rejection.
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "run on an uninstalled applet must be rejected");
    let err = run.error.unwrap();
    assert!(
        err.to_string().contains("lifecycle.applet_not_installed"),
        "carries the lifecycle.applet_not_installed marker: {err}"
    );
    assert!(err.to_string().contains("not installed"));
    assert_eq!(core.events().events_of_kind("run.started").count(), 0, "no user code ran");
    let rejected = core
        .events()
        .events_of_kind("runtime.run.rejected")
        .next()
        .expect("a runtime.run.rejected event is emitted");
    assert_eq!(rejected.payload["error_code"], serde_json::json!("lifecycle.applet_not_installed"));

    // enable / suspend / dispatch of the uninstalled applet are ALSO typed rejections.
    for (name, payload) in [
        ("applet.enable", serde_json::json!({})),
        ("applet.suspend", serde_json::json!({})),
        ("ui.dispatch_event", serde_json::json!({ "action_ref": "todo.add" })),
    ] {
        let resp = core.handle(cmd(name, Some("applet.todo"), payload));
        assert!(!resp.ok, "{name} on an uninstalled applet must be rejected");
        assert!(
            resp.error.unwrap().to_string().contains("not installed"),
            "{name} rejection names the not-installed state"
        );
    }
}

// ---------------------------------------------------------------------------
// reinstall_same_code_hash_noop: same code + same manifest over active = no-op
// ---------------------------------------------------------------------------

#[test]
fn reinstall_same_code_and_manifest_is_idempotent_noop() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let first = install(&mut core, todo_manifest());
    assert_eq!(first["version"], serde_json::json!(1));
    assert_eq!(first["install_generation"], serde_json::json!(1));

    // Reinstall the exact same sources + manifest.
    let again = install(&mut core, todo_manifest());
    assert_eq!(again["version"], serde_json::json!(1), "no new version is minted");
    assert_eq!(again["install_generation"], serde_json::json!(1), "same generation");
    assert_eq!(again["idempotent"], serde_json::json!(true));
    assert_eq!(again["code_hash"], first["code_hash"], "same code identity");
    assert_eq!(core.events().events_of_kind("applet.install.noop").count(), 1);
}

// ---------------------------------------------------------------------------
// uninstall_then_install_fresh_generation: gen 1 -> gen 2, version resets to 1
// ---------------------------------------------------------------------------

#[test]
fn uninstall_then_install_creates_fresh_generation() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let first = install(&mut core, todo_manifest());
    assert_eq!(first["install_generation"], serde_json::json!(1));
    seed_task(&mut core, "tasks/old", "retained");

    // Uninstall keep_data: the record survives, the active applet is removed.
    let uninstall = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "keep_data" }),
    ));
    assert!(uninstall.ok);

    // Reinstall: a fresh generation 2, version back to 1.
    let reinstall = install(&mut core, todo_manifest());
    assert_eq!(reinstall["install_generation"], serde_json::json!(2), "a fresh generation");
    assert_eq!(reinstall["version"], serde_json::json!(1), "version resets to 1 in the new generation");
    assert_eq!(reinstall["lifecycle"], serde_json::json!("enabled"));

    // The retained record from the prior generation is still present + live.
    let rec = core.store().get_record("tasks", "tasks/old").unwrap().expect("prior record retained");
    assert!(!rec.deleted);

    let installed: Vec<_> = core.events().events_of_kind("applet.installed").collect();
    assert_eq!(installed.len(), 2, "two installs across the two generations");
    assert_eq!(installed[1].payload["install_generation"], serde_json::json!(2));
}

// ---------------------------------------------------------------------------
// a same-code reinstall under a DIFFERENT manifest is NOT a no-op (it bumps
// version within the generation) — guards the idempotency-too-eager regression.
// ---------------------------------------------------------------------------

#[test]
fn reinstall_same_code_different_manifest_bumps_version() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let first = install(&mut core, todo_manifest());
    assert_eq!(first["version"], serde_json::json!(1));

    // Same sources, tighter limits → a real re-install (not idempotent).
    let mut tighter = todo_manifest();
    tighter["limits"]["log_bytes"] = serde_json::json!(1);
    let again = install(&mut core, tighter);
    assert_eq!(again["version"], serde_json::json!(2), "a manifest change bumps the version");
    assert_eq!(again["install_generation"], serde_json::json!(1), "same generation");
    assert!(again.get("idempotent").is_none(), "not flagged idempotent");
}

// ---------------------------------------------------------------------------
// A run recorded before uninstall still replays against its OWN pinned program/
// code_hash afterwards (spec: uninstall keeps replay artifacts for audit/replay).
// ---------------------------------------------------------------------------

#[test]
fn recorded_run_replays_after_uninstall_keep_data() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "the initial run must record: {:?}", run.error);
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();
    let recorded_code_hash = run.payload["code_hash"].as_str().unwrap().to_string();

    // Uninstall (keep_data) removes the ACTIVE applet — but NOT the run record or
    // the per-run pinned program.
    let uninstall = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "keep_data" }),
    ));
    assert!(uninstall.ok);
    // A fresh run is now rejected (no active applet).
    let blocked = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(!blocked.ok, "a new run after uninstall is rejected");

    // The recorded run STILL replays byte-identically against its own pinned program.
    let replay = core.handle(cmd_as(
        actor(Role::Auditor),
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(replay.ok, "the recorded run must replay after uninstall: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
    assert_eq!(
        run.payload["code_hash"], serde_json::json!(recorded_code_hash),
        "replay uses the run's own recorded code_hash"
    );
}

// ---------------------------------------------------------------------------
// Gate precedence: `uninstalled` (absence of an active record) takes priority
// over the dormant lifecycle flag. An applet suspended and THEN uninstalled
// (keep_data, so the flag survives) rejects run + dispatch with the
// not-installed marker — NOT the suspended one — on BOTH dispatch paths, so the
// two paths agree and do not depend on the flag being scrubbed on uninstall.
// ---------------------------------------------------------------------------

#[test]
fn uninstalled_takes_precedence_over_dormant_suspended_flag() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Suspend first (writes the Suspended flag), then uninstall keep_data — which
    // removes the active record but does not touch the lifecycle flag.
    core.handle(cmd("applet.suspend", Some("applet.todo"), serde_json::json!({})));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);
    let uninstall = core.handle(cmd(
        "applet.uninstall",
        Some("applet.todo"),
        serde_json::json!({ "retention_policy": "keep_data" }),
    ));
    assert!(uninstall.ok, "uninstall must succeed: {:?}", uninstall.error);
    // The dormant flag is still Suspended — proving the gates do NOT rely on it
    // being reset.
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);

    // runtime.run reports not-installed (uninstalled wins over the dormant flag).
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(!run.ok);
    assert!(
        run.error.unwrap().to_string().contains("lifecycle.applet_not_installed"),
        "run on an uninstalled-but-flagged-suspended applet reports not-installed, not suspended"
    );
    let run_rejected = core
        .events()
        .events_of_kind("runtime.run.rejected")
        .next()
        .expect("a runtime.run.rejected event is emitted");
    assert_eq!(
        run_rejected.payload["error_code"],
        serde_json::json!("lifecycle.applet_not_installed")
    );

    // ui.dispatch_event reports the same not-installed code on the same state.
    let dispatch = core.handle(cmd(
        "ui.dispatch_event",
        Some("applet.todo"),
        serde_json::json!({ "action_ref": "todo.add" }),
    ));
    assert!(!dispatch.ok);
    assert!(
        dispatch.error.unwrap().to_string().contains("lifecycle.applet_not_installed"),
        "dispatch agrees with run: not-installed, not suspended"
    );
    let dispatch_rejected = core
        .events()
        .events_of_kind("ui.dispatch_event.rejected")
        .next()
        .expect("a ui.dispatch_event.rejected event is emitted");
    assert_eq!(
        dispatch_rejected.payload["error_code"],
        serde_json::json!("lifecycle.applet_not_installed")
    );
    assert_eq!(
        dispatch_rejected.payload["code"],
        serde_json::json!("lifecycle.applet_not_installed")
    );
}

// ---------------------------------------------------------------------------
// RBAC: lifecycle administration is maintainer+ (a Viewer cannot suspend).
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_commands_require_maintainer_role() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());

    for name in ["applet.enable", "applet.suspend", "applet.upgrade", "applet.uninstall"] {
        let resp = core.handle(cmd_as(
            actor(Role::Viewer),
            name,
            Some("applet.todo"),
            serde_json::json!({ "retention_policy": "keep_data" }),
        ));
        assert!(!resp.ok, "a Viewer must not be permitted to {name}");
        assert_eq!(resp.error.unwrap().code(), "PermissionDenied");
    }
    // The applet is unchanged: still installed + enabled.
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Active);
}

// ---------------------------------------------------------------------------
// applet.upgrade: a successful upgrade does NOT resume a suspended applet
// (spec line 97 — the new active version inherits the prior lifecycle flag).
// ---------------------------------------------------------------------------

#[test]
fn upgrade_does_not_resume_a_suspended_applet() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());
    // Suspend, then upgrade to v2 (distinct source ⇒ distinct code_hash).
    core.handle(cmd("applet.suspend", Some("applet.todo"), serde_json::json!({})));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);

    let upgrade = core.handle(cmd(
        "applet.upgrade",
        Some("applet.todo"),
        serde_json::json!({ "manifest": todo_manifest(), "sources": { "src/main.ts": V2_TS } }),
    ));
    assert!(upgrade.ok, "upgrade of a suspended applet must succeed: {:?}", upgrade.error);
    assert_eq!(upgrade.payload["version"], serde_json::json!(2), "the active version is v2");
    // The new active version stays SUSPENDED (no implicit resume).
    assert_eq!(upgrade.payload["state"], serde_json::json!("suspended"));
    assert_eq!(core.applet_lifecycle("applet.todo").unwrap(), AppletLifecycle::Suspended);
    // A run is still rejected (the upgrade did not resume the applet).
    let run = core.handle(cmd("runtime.run", Some("applet.todo"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "a suspended-then-upgraded applet still rejects runs");
    assert!(run.error.unwrap().to_string().contains("lifecycle.applet_suspended"));
    // The upgrade audit names the post-state as suspended.
    let ev = core.events().events_of_kind("applet.upgraded").next().expect("applet.upgraded");
    assert_eq!(ev.payload["state_after"], serde_json::json!("suspended"));
}

// ---------------------------------------------------------------------------
// applet.upgrade of an uninstalled applet is a typed not-installed rejection
// (not an upgrade-failed) — the illegal transition is fail-closed.
// ---------------------------------------------------------------------------

#[test]
fn upgrade_uninstalled_is_not_installed_rejection() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Never installed.
    let upgrade = core.handle(cmd(
        "applet.upgrade",
        Some("applet.todo"),
        serde_json::json!({ "manifest": todo_manifest(), "sources": { "src/main.ts": V2_TS } }),
    ));
    assert!(!upgrade.ok, "upgrading an uninstalled applet must be rejected");
    let err = upgrade.error.unwrap();
    assert!(
        err.to_string().contains("lifecycle.applet_not_installed"),
        "carries the not-installed marker (not upgrade_failed): {err}"
    );
    // No version was minted; no upgrade events.
    assert_eq!(core.events().events_of_kind("applet.upgraded").count(), 0);
    assert_eq!(core.events().events_of_kind("applet.upgrade.rejected").count(), 0);
}

// ---------------------------------------------------------------------------
// applet.upgrade with an identical payload is NOT an upgrade (same-payload
// reinstall is the applet.install no-op path) — it is rejected at the staged
// compile boundary, leaving the active version untouched.
// ---------------------------------------------------------------------------

#[test]
fn upgrade_with_identical_payload_is_rejected_not_a_spurious_v2() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install(&mut core, todo_manifest());

    let upgrade = core.handle(cmd(
        "applet.upgrade",
        Some("applet.todo"),
        serde_json::json!({ "manifest": todo_manifest(), "sources": { "src/main.ts": TODO_TS } }),
    ));
    assert!(!upgrade.ok, "an identical-payload upgrade is rejected (it is an install no-op, not an upgrade)");
    // Still v1: a same-code reinstall reports the existing version 1, idempotent.
    let probe = install(&mut core, todo_manifest());
    assert_eq!(probe["version"], serde_json::json!(1), "no spurious v2 was minted");
    assert_eq!(probe["idempotent"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Seed a live `tasks` record the applet owns (collection in its `db.write`
/// grant), directly through the store, so the uninstall retention paths have an
/// applet-owned record to retain/tombstone.
fn seed_task(core: &mut WorkspaceCore, id: &str, title: &str) {
    let env = forge_domain::RecordEnvelope::new(
        forge_domain::CollectionId::new("tasks"),
        forge_domain::RecordId::new(id),
        [("title".to_string(), serde_json::json!(title))].into_iter().collect(),
        forge_domain::LogicalTimestamp(1),
    );
    core.store_mut().put_record(&env).unwrap();
}
