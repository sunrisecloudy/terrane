//! LIVE-WIRING proof for the SC-10 trusted-source gates (T037 FIX ROUND 2).
//!
//! These tests drive the REAL production caller — `WorkspaceCore::handle` →
//! `cmd_runtime_run` / `cmd_ui_dispatch_event` — and prove that a trusted
//! workspace-policy / run-profile / platform-permission deny configured via
//! `set_run_policy` actually BLOCKS a live command. They are the executable
//! evidence that the gates are on the live decision path, not a tested-but-
//! disconnected library: no test here installs a `DecisionContext` by hand — the
//! deny is configured purely as trusted workspace state and the run is issued the
//! same way a shell would.
//!
//! `spec/policy-gates.md` (gates 2/4/5) is the contract; the per-gate unit/vector
//! coverage lives in `forge-policy`. This file pins the *wiring*.

use forge_core::{Capability, RunPolicy, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};

/// A demo applet whose FIRST effect is `ctx.db.insert` (db category) and which
/// then writes storage + renders ui — so a deny on any of the db/storage/ui
/// categories fails the run at the corresponding host call.
const DEMO_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const id = await ctx.db.insert("tasks", { title: "live", done: false });
        await ctx.storage.set("app/last", { id: id });
        await ctx.ui.render({ type: "Text", text: "ok" });
        return { ok: true, value: { id: id } };
    }
"#;

fn demo_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": ["app/*"], "write": ["app/*"] },
            "db": { "read": ["tasks"], "write": ["tasks"] },
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

fn install_demo(core: &mut WorkspaceCore) {
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_demo"),
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": DEMO_TS }
        }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);
}

fn run(core: &mut WorkspaceCore) -> forge_domain::CoreResponse {
    core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ))
}

/// BASELINE: an un-provisioned workspace (no `set_run_policy`) runs under the
/// permissive `AllowAll` context — the M0a spine default. This proves the live
/// gates default-open when unconfigured, so wiring them did not regress the spine.
#[test]
fn unprovisioned_workspace_runs_under_allow_all() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);
    assert!(core.run_policy().is_none(), "no policy configured");

    let resp = run(&mut core);
    assert!(resp.ok, "run command completes: {:?}", resp.error);
    assert_eq!(resp.payload["ok"], serde_json::json!(true), "run must succeed: {:?}", resp.payload);
    assert!(
        core.store().get_record("tasks", "tasks/1").unwrap().is_some(),
        "the db.insert landed"
    );
}

/// LIVE PROOF #1 — workspace-policy gate (gate 2). A trusted `RunPolicy` that
/// forbids the `db` category blocks the live `ctx.db.insert` with a
/// `PermissionDenied` naming the workspace-policy gate — and NO record is written.
/// The deny is configured purely as trusted workspace state; the run is issued
/// through the real `runtime.run` command.
#[test]
fn workspace_policy_deny_blocks_live_runtime_run() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);

    // Trusted workspace admin policy forbids the `db` capability category.
    core.set_run_policy(RunPolicy {
        workspace_denied: vec![Capability::Db],
        ..RunPolicy::default()
    })
    .unwrap();

    let resp = run(&mut core);
    assert!(resp.ok, "the run command itself completes (records the failure)");
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "run OUTCOME is a denial");

    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("PermissionDenied"),
        "denial must surface as PermissionDenied: {result_str}"
    );
    assert!(
        result_str.contains("workspace policy"),
        "the surfaced gate must be the workspace-policy gate: {result_str}"
    );

    // The deny actually blocked the effect: NO record landed.
    assert!(
        core.store().get_record("tasks", "tasks/1").unwrap().is_none(),
        "a workspace-policy deny must block the live db.insert — no record written"
    );
    assert!(core.store().list_records("tasks").unwrap().is_empty());
}

/// LIVE PROOF #2 — run-profile gate (gate 4). A trusted run profile whose bounds
/// exclude `db` blocks the live command at the run-profile gate.
#[test]
fn run_profile_exclusion_blocks_live_runtime_run() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);

    // The run profile permits storage/ui/time/random but NOT db.
    core.set_run_policy(RunPolicy {
        run_profile_name: Some("review-safety".to_string()),
        run_profile_permitted: Some(vec![
            Capability::Storage,
            Capability::Ui,
            Capability::Time,
            Capability::Random,
        ]),
        ..RunPolicy::default()
    })
    .unwrap();

    let resp = run(&mut core);
    assert_eq!(resp.payload["ok"], serde_json::json!(false));
    let result_str = resp.payload["result"].to_string();
    assert!(result_str.contains("PermissionDenied"), "{result_str}");
    assert!(result_str.contains("run profile"), "names the run-profile gate: {result_str}");
    assert!(result_str.contains("review-safety"), "names the profile: {result_str}");
    assert!(core.store().get_record("tasks", "tasks/1").unwrap().is_none());
}

/// LIVE PROOF #3 — platform-permission gate (gate 5). A trusted platform grant
/// set that omits `db` makes the capability UNAVAILABLE (not refused) on the live
/// path: the run fails with `PlatformUnavailable`.
#[test]
fn platform_unavailable_blocks_live_runtime_run() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);

    // The OS grants storage/ui/time/random but NOT db.
    core.set_run_policy(RunPolicy {
        platform_granted: Some(vec![
            Capability::Storage,
            Capability::Ui,
            Capability::Time,
            Capability::Random,
        ]),
        ..RunPolicy::default()
    })
    .unwrap();

    let resp = run(&mut core);
    assert_eq!(resp.payload["ok"], serde_json::json!(false));
    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("PlatformUnavailable"),
        "an OS-ungranted capability is unavailable, not refused: {result_str}"
    );
    assert!(result_str.contains("platform permission"), "names the platform gate: {result_str}");
    assert!(core.store().get_record("tasks", "tasks/1").unwrap().is_none());
}

/// A PARTIAL policy only restricts the gate the admin configured: denying only
/// `ui` still lets the (earlier) db + storage calls through, and the run fails
/// ONLY when it reaches `ctx.ui.render`. Proves the unspecified gates default to
/// "allow all" (the policy only ADDS denials relative to AllowAll).
#[test]
fn partial_policy_restricts_only_the_configured_gate() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);

    core.set_run_policy(RunPolicy {
        workspace_denied: vec![Capability::Ui],
        ..RunPolicy::default()
    })
    .unwrap();

    let resp = run(&mut core);
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "the ui.render is denied");
    let result_str = resp.payload["result"].to_string();
    assert!(result_str.contains("workspace policy"), "{result_str}");

    // The db.insert and storage.set BEFORE the ui.render were NOT denied — they ran.
    assert!(
        core.store().get_record("tasks", "tasks/1").unwrap().is_some(),
        "the db.insert preceding the denied ui.render still landed"
    );
}

/// The configured policy is PERSISTED to the workspace file (like `db_read_grants`
/// / `sync_membership`): a deny survives reopening the file-backed workspace, so a
/// re-opened workspace still blocks the live command (no fail-open on reopen).
#[test]
fn run_policy_deny_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws.forge");

    {
        let mut core = WorkspaceCore::open(&path, "ws1").unwrap();
        install_demo(&mut core);
        core.set_run_policy(RunPolicy {
            workspace_denied: vec![Capability::Db],
            ..RunPolicy::default()
        })
        .unwrap();
        // Pre-reopen: the deny blocks the live run.
        let resp = run(&mut core);
        assert_eq!(resp.payload["ok"], serde_json::json!(false));
    }

    // Reopen the SAME file: the persisted policy is loaded, so the deny still fires.
    let mut reopened = WorkspaceCore::open(&path, "ws1").unwrap();
    assert!(reopened.run_policy().is_some(), "the policy persisted across reopen");
    let resp = run(&mut reopened);
    assert_eq!(
        resp.payload["ok"],
        serde_json::json!(false),
        "a persisted workspace-policy deny still blocks the live run after reopen"
    );
    let result_str = resp.payload["result"].to_string();
    assert!(result_str.contains("workspace policy"), "{result_str}");
}
