//! End-to-end spine integration tests for `forge-core`.
//!
//! These tests are THE proof of the M0a jewel (prd-merged/01 CR-A1..A5, CR-8,
//! CR-9, CR-13/CR-14; prd-merged/05 UI-1): a real TypeScript applet is installed
//! (TS → SWC → policy scan → store), run (QuickJS → capability-checked ctx →
//! SQLite write → UI patch → recorded RunRecord), replayed byte-identically, and
//! denied/​rejected on the unhappy paths — all offline, against the real Store.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, ActorId, AppletId, CoreCommand, RequestId, Role, WorkspaceId};

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
    cmd_as(owner(), name, applet_id, payload)
}

/// Like [`cmd`] but with an explicit actor, for the command-level RBAC tests
/// (review 031 finding 1).
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

/// An actor in `role` (id derived from the role for readable failures).
fn actor(role: Role) -> ActorContext {
    ActorContext { actor: ActorId::new(format!("{role:?}").to_lowercase()), role }
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

// ---------------------------------------------------------------------------
// 5. command-level RBAC (review 031 finding 1, CR-A3): the actor role is gated
//    per command BEFORE dispatch, matching forge/spec/commands.md.
// ---------------------------------------------------------------------------

/// A Viewer (read-only) cannot install an applet; the gate fires before any
/// compile/store, and a follow-up run reports the applet missing.
#[test]
fn viewer_cannot_install_applet() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let resp = core.handle(cmd_as(
        actor(Role::Viewer),
        "applet.install",
        Some("app_demo"),
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": DEMO_TS }
        }),
    ));
    assert!(!resp.ok, "Viewer install must be denied");
    assert_eq!(resp.error.unwrap().code(), "PermissionDenied");

    // Nothing was installed: an owner run now reports it missing (the gate ran
    // before the store was touched).
    let run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(!run.ok);
    assert_eq!(run.error.unwrap().code(), "ValidationError");
}

/// An Auditor cannot run code, and a Viewer cannot run code: `runtime.run` is
/// limited to the run-capable roles.
#[test]
fn read_only_roles_cannot_run() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    for role in [Role::Viewer, Role::Auditor] {
        let resp = core.handle(cmd_as(
            actor(role),
            "runtime.run",
            Some("app_demo"),
            serde_json::json!({ "input": {} }),
        ));
        assert!(!resp.ok, "{role:?} must not be permitted to runtime.run");
        assert_eq!(resp.error.unwrap().code(), "PermissionDenied");
    }
    // And no run was recorded by the denied attempts.
    assert_eq!(core.events().events_of_kind("run.started").count(), 0);
}

/// A bare Runner can run but NOT replay (commands.md: replay is Auditor /
/// Maintainer / Owner). The Runner records a run; replaying it as a Runner is
/// denied; replaying as an Auditor succeeds.
#[test]
fn runner_can_run_but_not_replay_auditor_can_replay() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    // Runner runs (allowed).
    let run = core.handle(cmd_as(
        actor(Role::Runner),
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Run by runner" } }),
    ));
    assert!(run.ok, "Runner must be permitted to runtime.run: {:?}", run.error);
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();

    // Runner replay (denied by the command-level gate).
    let denied = core.handle(cmd_as(
        actor(Role::Runner),
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(!denied.ok, "Runner must not be permitted to runtime.replay");
    assert_eq!(denied.error.unwrap().code(), "PermissionDenied");

    // Auditor replay (allowed) and replays identically.
    let ok = core.handle(cmd_as(
        actor(Role::Auditor),
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(ok.ok, "Auditor must be permitted to replay: {:?}", ok.error);
    assert_eq!(ok.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// 6. unique per-execution run identity (review 031 finding 2, CR-9): two runs
//    of the same applet with DIFFERENT inputs both persist + remain loadable.
// ---------------------------------------------------------------------------

#[test]
fn two_runs_persist_distinctly_and_each_replays_identically() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    let r1 = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "First" } })));
    assert!(r1.ok, "{:?}", r1.error);
    let id1 = r1.payload["run_id"].as_str().unwrap().to_string();

    let r2 = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "Second" } })));
    assert!(r2.ok, "{:?}", r2.error);
    let id2 = r2.payload["run_id"].as_str().unwrap().to_string();

    // Distinct run ids → the second did not overwrite the first.
    assert_ne!(id1, id2, "each execution must mint a unique run_id");

    // BOTH records remain independently loadable.
    let rec1 = core.store().load_run(&id1).unwrap().expect("first run must persist");
    let rec2 = core.store().load_run(&id2).unwrap().expect("second run must persist");
    // They captured their distinct inputs.
    assert_eq!(rec1.input["title"], serde_json::json!("First"));
    assert_eq!(rec2.input["title"], serde_json::json!("Second"));

    // Each replays identically to ITSELF (deterministic across independent runs).
    for id in [&id1, &id2] {
        let resp = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": id })));
        assert!(resp.ok, "replay of {id} must succeed: {:?}", resp.error);
        assert_eq!(resp.payload["replays_identically"], serde_json::json!(true));
    }
}

/// Re-running the SAME (stateless) applet with the SAME input keeps the
/// deterministic replay seeds — and because the applet's trace depends only on
/// seeds+input (it does not mutate shared DB state), the two records are
/// replay-identical to EACH OTHER — while still persisting as two distinct,
/// loadable runs (unique run_id). This is the "deterministic across independent
/// runs" property the unique run_id must not break (review 031 finding 2).
#[test]
fn same_input_reruns_share_seeds_but_persist_separately() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // A pure, stateless applet: its trace is a function of seeds + input only
    // (it consumes the random/time seams but writes no shared state), so two
    // runs with the same input replay-identically to each other.
    let pure_ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            const r = await ctx.random.next();
            const t = await ctx.time.now();
            await ctx.ui.render({ type: "Text", text: "pure" });
            return { ok: true, value: { r: r, t: t } };
        }
    "#;
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": { "storage": { "read": [], "write": [] }, "db": { "read": [], "write": [] }, "ui": true },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    install_demo(&mut core, pure_ts, manifest);

    let input = serde_json::json!({ "input": { "title": "Same" } });
    let a = core.handle(cmd("runtime.run", Some("app_demo"), input.clone()));
    let b = core.handle(cmd("runtime.run", Some("app_demo"), input));
    assert!(a.ok && b.ok, "both runs must succeed: {:?} {:?}", a.error, b.error);
    let ida = a.payload["run_id"].as_str().unwrap().to_string();
    let idb = b.payload["run_id"].as_str().unwrap().to_string();
    assert_ne!(ida, idb, "same input still mints distinct run_ids");

    let reca = core.store().load_run(&ida).unwrap().unwrap();
    let recb = core.store().load_run(&idb).unwrap().unwrap();
    // Deterministic seeds derived from (code_hash, input) → equal across re-runs.
    assert_eq!(reca.random_seed, recb.random_seed);
    assert_eq!(reca.time_start, recb.time_start);
    // ...and the two records are replay-identical to EACH OTHER (run_id excluded).
    assert!(reca.replays_identically(&recb), "same code+input runs must be replay-identical");
}

// ---------------------------------------------------------------------------
// 7. version-pinned replay (review 031 finding 3, CR-9): install v1, run,
//    reinstall v2 (different code), replay the v1 run → still replays
//    identically against v1's recorded program (code_hash).
// ---------------------------------------------------------------------------

#[test]
fn replay_uses_recorded_program_not_reinstalled_version() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // v1: returns value "v1".
    let v1_ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            await ctx.ui.render({ type: "Text", text: "v1" });
            return { ok: true, value: "v1" };
        }
    "#;
    install_demo(&mut core, v1_ts, demo_manifest());
    let run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "v1 run must succeed: {:?}", run.error);
    let v1_run_id = run.payload["run_id"].as_str().unwrap().to_string();
    let v1_code_hash = run.payload["code_hash"].as_str().unwrap().to_string();

    // v2: DIFFERENT code (returns "v2"), reinstalled over the same applet id.
    let v2_ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            await ctx.ui.render({ type: "Text", text: "COMPLETELY DIFFERENT v2" });
            return { ok: true, value: "v2-and-more" };
        }
    "#;
    install_demo(&mut core, v2_ts, demo_manifest());
    let v2_hash = forge_pipeline::compile(v2_ts).unwrap().code_hash;
    assert_ne!(v1_code_hash, v2_hash, "v2 must be different code than v1");

    // Replaying the v1 run reconstructs v1's program from the recorded code_hash,
    // NOT the currently installed v2 — so it still replays byte-identically.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": v1_run_id })));
    assert!(replay.ok, "v1 replay after v2 reinstall must succeed: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));

    // The replayed run is provably the v1 program (its code_hash), not v2's.
    let v1_rec = core.store().load_run(&v1_run_id).unwrap().unwrap();
    assert_eq!(v1_rec.code_hash, v1_code_hash);
    assert_ne!(v1_rec.code_hash, v2_hash);
}
