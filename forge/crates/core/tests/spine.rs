//! End-to-end spine integration tests for `forge-core`.
//!
//! These tests are THE proof of the M0a jewel (prd-merged/01 CR-A1..A5, CR-8,
//! CR-9, CR-13/CR-14; prd-merged/05 UI-1): a real TypeScript applet is installed
//! (TS → SWC → policy scan → store), run (QuickJS → capability-checked ctx →
//! SQLite write → UI patch → recorded RunRecord), replayed byte-identically, and
//! denied/​rejected on the unhappy paths — all offline, against the real Store.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};

/// A small inline TS applet that exercises all three effect families: it writes
/// a record (`ctx.db.insert`), writes KV (`ctx.storage.set`), and renders a UI
/// tree (`ctx.ui.render` of a Stack containing a Text and a List). The host
/// surface is async (returns Promises), so it `await`s each call.
const DEMO_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const title: string = input && input.title ? input.title : "Ship M0a";
        const id = await ctx.db.insert("tasks", { title: title, done: false });
        await ctx.storage.set("app/last", { id: id });
        ctx.log("rendered task " + id);
        await ctx.ui.render({
            type: "Stack",
            direction: "v",
            children: [
                { type: "Text", text: "Tasks" },
                { type: "List", items: [ { type: "Text", text: title } ] }
            ]
        });
        return { ok: true, value: { id: id } };
    }
"#;

/// Manifest JSON granting db write to `tasks`, storage write to `app/*`, and ui.
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

/// Install the demo applet into a fresh in-memory workspace, asserting success.
fn install_demo(core: &mut WorkspaceCore, ts: &str, manifest: serde_json::Value) {
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_demo"),
        serde_json::json!({
            "manifest": manifest,
            "sources": { "src/main.ts": ts }
        }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);
}

// ---------------------------------------------------------------------------
// 1. install + run: a record is written, UI patches emitted, run saved.
// ---------------------------------------------------------------------------

#[test]
fn install_run_writes_record_emits_ui_patch_and_saves_run() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Buy milk" } }),
    ));
    assert!(resp.ok, "run must succeed: {:?}", resp.error);
    assert_eq!(resp.payload["ok"], serde_json::json!(true));
    let run_id = resp.payload["run_id"].as_str().expect("run_id").to_string();

    // A record is ACTUALLY in the Store (get_record / list_records).
    let rec = core
        .store()
        .get_record("tasks", "tasks/1")
        .unwrap()
        .expect("record must be written to the records projection");
    assert_eq!(rec.fields["title"], serde_json::json!("Buy milk"));
    assert_eq!(rec.fields["done"], serde_json::json!(false));
    assert_eq!(core.store().list_records("tasks").unwrap().len(), 1);

    // The KV write also landed (scoped to the applet namespace).
    let last = core
        .store()
        .kv_get("applet/app_demo", "app/last")
        .unwrap()
        .expect("storage.set must persist");
    let last_json: serde_json::Value = serde_json::from_slice(&last).unwrap();
    assert_eq!(last_json["id"], serde_json::json!("tasks/1"));

    // UI patch events were emitted, non-empty, and the tree carries the rendered
    // Text/List content.
    let ui_patches: Vec<_> = core.events().events_of_kind("ui.patch").collect();
    assert_eq!(ui_patches.len(), 1, "exactly one ui.render → one ui.patch event");
    let patch_event = ui_patches[0];
    let tree = &patch_event.payload["tree"];
    let tree_str = tree.to_string();
    assert!(tree_str.contains("\"Tasks\""), "rendered Text present: {tree_str}");
    assert!(tree_str.contains("\"Buy milk\""), "list item text present: {tree_str}");
    assert!(tree_str.contains("\"List\""), "List node present: {tree_str}");
    // First render diffs against None → a single root replace patch.
    let patches = &patch_event.payload["patches"];
    assert!(patches.is_array() && !patches.as_array().unwrap().is_empty());
    assert_eq!(patches[0]["op"], serde_json::json!("replace"));

    // run.started + run.completed events were emitted.
    assert_eq!(core.events().events_of_kind("run.started").count(), 1);
    assert_eq!(core.events().events_of_kind("run.completed").count(), 1);

    // The RunRecord was saved (load it back).
    let saved = core.store().load_run(&run_id).unwrap();
    assert!(saved.is_some(), "RunRecord must be persisted for replay");
    assert!(saved.unwrap().is_completed());
}

// ---------------------------------------------------------------------------
// CODE_HASH UNIFICATION: the stored RunRecord.code_hash equals the pipeline's
// sha256 hash for the demo applet — proving the TS → SWC → run provenance chain.
// ---------------------------------------------------------------------------

#[test]
fn run_record_code_hash_equals_pipeline_sha256_hash() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Independently compile the demo source with the pipeline to get the SAME
    // canonical sha256 hash the install/run path uses.
    let pipeline_program = forge_pipeline::compile(DEMO_TS).unwrap();
    assert!(
        pipeline_program.code_hash.starts_with("sha256:"),
        "pipeline must emit canonical sha256: hash"
    );
    assert!(forge_domain::is_canonical_code_hash(&pipeline_program.code_hash));

    install_demo(&mut core, DEMO_TS, demo_manifest());
    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    assert!(resp.ok, "run failed: {:?}", resp.error);
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    let saved = core.store().load_run(&run_id).unwrap().unwrap();
    assert_eq!(
        saved.code_hash, pipeline_program.code_hash,
        "stored RunRecord.code_hash must equal the pipeline-produced sha256 hash \
         (TS → SWC → run provenance)"
    );
    // And the install/run response surfaced the same hash.
    assert_eq!(resp.payload["code_hash"], serde_json::json!(pipeline_program.code_hash));
}

// ---------------------------------------------------------------------------
// 2. replay: identical fingerprint; tampering the saved run → divergence.
// ---------------------------------------------------------------------------

#[test]
fn replay_is_identical_to_original() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());
    let run_resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Replay me" } }),
    ));
    assert!(run_resp.ok, "run failed: {:?}", run_resp.error);
    let run_id = run_resp.payload["run_id"].as_str().unwrap().to_string();

    let original = core.store().load_run(&run_id).unwrap().unwrap();

    let replay_resp = core.handle(cmd(
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(replay_resp.ok, "replay must succeed: {:?}", replay_resp.error);
    assert_eq!(replay_resp.payload["ok"], serde_json::json!(true));
    assert_eq!(replay_resp.payload["replays_identically"], serde_json::json!(true));
    // The replay fingerprint equals the original's.
    assert_eq!(
        replay_resp.payload["fingerprint"],
        serde_json::json!(original.replay_fingerprint())
    );
    assert_eq!(core.events().events_of_kind("run.replayed").count(), 1);
}

#[test]
fn replay_of_tampered_run_reports_divergence() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());
    let run_resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    let run_id = run_resp.payload["run_id"].as_str().unwrap().to_string();

    // Tamper with the saved run's recorded response (the db.insert ack), then
    // re-save it directly via the Store. Replay must detect the divergence.
    let mut tampered = core.store().load_run(&run_id).unwrap().unwrap();
    let insert_idx = tampered
        .calls
        .iter()
        .position(|c| c.method == "db.insert")
        .expect("the run recorded a db.insert");
    tampered.calls[insert_idx].response = serde_json::json!("tasks/tampered");
    // Re-save the tampered record (code_hash unchanged → still canonical).
    {
        // Re-open behavior: save_run overwrites by run_id.
        // We need &mut Store; go through a fresh handle by reusing the public
        // store-mutating path is not exposed, so persist via the run command's
        // store. Use a direct save through a helper core method shim:
        tamper_save(&mut core, &tampered);
    }

    let replay_resp = core.handle(cmd(
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(!replay_resp.ok, "tampered replay must fail");
    let err = replay_resp.error.expect("replay must surface an error");
    assert_eq!(err.code(), "RuntimeError", "divergence is a RuntimeError: {err}");
}

/// Persist a (tampered) run directly into the workspace store. Exercises the
/// same `Store::save_run` the run command uses, via a test-only accessor.
fn tamper_save(core: &mut WorkspaceCore, run: &forge_domain::RunRecord) {
    core.store_mut().save_run(run).unwrap();
}

// ---------------------------------------------------------------------------
// 3. capability denial: a manifest lacking db write → PermissionDenied, no write.
// ---------------------------------------------------------------------------

#[test]
fn run_without_db_write_capability_is_permission_denied_and_writes_nothing() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Manifest grants storage + ui but NOT db.write for `tasks`.
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": ["app/*"], "write": ["app/*"] },
            "db": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    install_demo(&mut core, DEMO_TS, manifest);

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    // The run command itself returns ok:true (the run was recorded), but the run
    // OUTCOME is a failure surfaced as the AppResult/run.failed path. The denial
    // is the run's error outcome.
    assert!(resp.ok, "the run.command itself completes (records the failure)");
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "run outcome must be a failure");

    // The error is a capability denial (the applet declared NO db capability, so
    // the gate is CapabilityRequired; either denial code proves the gate fired).
    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("CapabilityRequired") || result_str.contains("PermissionDenied"),
        "denial must surface in the run outcome: {result_str}"
    );

    // NO record was written.
    assert!(
        core.store().get_record("tasks", "tasks/1").unwrap().is_none(),
        "a denied db.insert must not write a record"
    );
    assert!(core.store().list_records("tasks").unwrap().is_empty());

    // A run.failed event was emitted, not run.completed.
    assert_eq!(core.events().events_of_kind("run.failed").count(), 1);
    assert_eq!(core.events().events_of_kind("run.completed").count(), 0);
}

// ---------------------------------------------------------------------------
// 4. install rejects forbidden source (eval) → error response, not installed.
// ---------------------------------------------------------------------------

#[test]
fn install_rejects_source_with_eval_and_does_not_install() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let evil_ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            const x = eval("1 + 1");
            return { ok: true, value: x };
        }
    "#;
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_evil"),
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": evil_ts }
        }),
    ));
    assert!(!resp.ok, "install of a source containing eval( must fail");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "PermissionDenied", "policy scan rejects eval: {err}");

    // The applet was NOT installed: a subsequent run reports it missing.
    let run = core.handle(cmd(
        "runtime.run",
        Some("app_evil"),
        serde_json::json!({ "input": {} }),
    ));
    assert!(!run.ok);
    assert_eq!(run.error.unwrap().code(), "ValidationError");
}

// ---------------------------------------------------------------------------
// query.execute reads the records projection back.
// ---------------------------------------------------------------------------

#[test]
fn query_execute_lists_written_records() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());
    core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "A" } })));
    core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "B" } })));

    let resp = core.handle(cmd(
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(resp.ok, "{:?}", resp.error);
    let rows = resp.payload["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both runs wrote a record");
    let titles: Vec<&str> = rows
        .iter()
        .map(|r| r["fields"]["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"A") && titles.contains(&"B"));
}

// ---------------------------------------------------------------------------
// workspace.create / workspace.open smoke.
// ---------------------------------------------------------------------------

#[test]
fn workspace_create_and_open_report_identity() {
    let mut core = WorkspaceCore::in_memory("ws_smoke").unwrap();
    let create = core.handle(cmd("workspace.create", None, serde_json::Value::Null));
    assert!(create.ok);
    assert_eq!(create.payload["workspace_id"], serde_json::json!("ws_smoke"));

    let open = core.handle(cmd("workspace.open", None, serde_json::json!({ "workspace_id": "ws_smoke" })));
    assert!(open.ok);
    assert_eq!(open.payload["workspace_id"], serde_json::json!("ws_smoke"));
}

#[test]
fn unknown_command_is_a_graceful_validation_error() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let resp = core.handle(cmd("does.not.exist", None, serde_json::Value::Null));
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code(), "ValidationError");
}
