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

/// Load a normative query fixture (`forge/fixtures/query/<name>`). The fixtures
/// are load-bearing across crates: forge-storage pins the unguarded-scan
/// boundary, and forge-core (here) pins the *caller* boundary that actually
/// enforces the `db.read` grant the fixture's `expect_error` describes.
fn load_query_fixture(name: &str) -> serde_json::Value {
    // CARGO_MANIFEST_DIR = forge/crates/core
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/query")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read query fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse query fixture {name}: {e}"))
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

/// Review 036 finding 1 (`forge/spec/commands.md:21` "Role plus db.read
/// capability"): a role that lacks `db.read` cannot read the records projection,
/// even though the command-level role gate is necessary. A `Runner` is
/// execution-only — it may `runtime.run` but is NOT a data reader — so its
/// `query.execute` is denied with `PermissionDenied` BEFORE any records are
/// listed. A `db.read`-capable role (Viewer) on the same workspace succeeds, so
/// the gate denies the capability, not the command.
#[test]
fn query_execute_requires_db_read_capability() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());
    // Seed a record so a successful query would actually return rows (proving the
    // denial is the capability gate, not an empty projection).
    core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "X" } })));

    // A Runner can run code but lacks db.read → query.execute is denied.
    let denied = core.handle(cmd_as(
        actor(Role::Runner),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(!denied.ok, "a Runner lacks db.read and must not query");
    assert_eq!(denied.error.unwrap().code(), "PermissionDenied");

    // A Viewer holds db.read → the same query on the same workspace succeeds.
    let ok = core.handle(cmd_as(
        actor(Role::Viewer),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(ok.ok, "Viewer holds db.read and must be permitted: {:?}", ok.error);
    assert_eq!(ok.payload["rows"].as_array().unwrap().len(), 1);
}

/// Review 038 finding 1 + `forge/fixtures/query/reject_ungranted_collection.json`
/// (`forge/spec/capabilities.md:23` — `db.read` is *collection-scoped*): the
/// `db.read` capability must be enforced against an actual GRANT SCOPE, not just
/// the role gate. An actor whose role clears the role allowlist (here an Owner,
/// the most-privileged role) but whose granted `db.read` scope does NOT include
/// the target collection is denied with `CapabilityRequired` — proving the
/// capability layer is load-bearing and distinct from the role gate (which the
/// owner passes). The fixture pins the exact grant shape and error.
///
/// Review 048 finding 1: the grant scope is the TRUSTED, workspace-side grant
/// (provisioned via `grant_db_read`), NOT the request payload. The request below
/// carries NO `grants` field at all, and the denial still fires — so the boundary
/// is trusted, not caller-supplied. The companion test
/// [`query_execute_db_read_scope_is_not_forgeable_from_payload`] proves a payload
/// cannot self-grant past the trusted scope.
#[test]
fn query_execute_enforces_collection_scoped_db_read_grant() {
    let fx = load_query_fixture("reject_ungranted_collection.json");
    // The fixture's grant: db.read scoped to ["tasks"] only.
    let grants = fx["grants"].clone();
    assert_eq!(grants["db"]["read"], serde_json::json!(["tasks"]));
    let target = fx["query"]["from"].as_str().unwrap().to_string(); // "secrets"
    let expect_code = fx["expect_error"]["code"].as_str().unwrap().to_string();
    let expect_msg = fx["expect_error"]["message_contains"].as_str().unwrap().to_string();

    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Provision the fixture's grant trustedly: the owner actor id is "dev".
    core.grant_db_read("dev", ["tasks"]).unwrap();

    // An Owner (clears the role gate) querying a collection OUTSIDE the granted
    // db.read scope is denied by the capability layer, before any scan — and the
    // request carries NO grants payload, so the scope came from the trusted table.
    let denied = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": target }),
    ));
    assert!(!denied.ok, "an out-of-scope db.read must be denied even for an Owner");
    let err = denied.error.expect("must carry an error");
    assert_eq!(err.code(), expect_code, "fixture pins {expect_code}: {err}");
    assert!(
        err.to_string().contains(&expect_msg),
        "error must name the ungranted scope {expect_msg:?}: {err}"
    );

    // The SAME owner, querying a collection that IS within the trusted scope,
    // succeeds — so the gate denies the out-of-scope capability, not the command.
    let in_scope = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(in_scope.ok, "an in-scope db.read must be permitted: {:?}", in_scope.error);
}

/// Review 050: a trusted `db.read` scope provisioned via `grant_db_read` must
/// SURVIVE reopening the file-backed workspace. Before the fix the grant table
/// was in-memory only, so after `open(...)` the actor had no entry and absence
/// meant role-derived read-all — a scoped actor could suddenly read `secrets`.
#[test]
fn db_read_grant_scope_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws.forge");

    // Provision a scoped grant, then drop the handle (simulating app restart).
    {
        let mut core = WorkspaceCore::open(&path, "ws1").unwrap();
        core.grant_db_read("dev", ["tasks"]).unwrap();
    }

    // Reopen the SAME file: the scoped grant must still be in force.
    let mut core = WorkspaceCore::open(&path, "ws1").unwrap();

    // An ungranted collection stays DENIED after reopen (no fail-open to read-all).
    let denied = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "secrets" }),
    ));
    assert!(
        !denied.ok,
        "a scoped db.read must remain scoped after reopen — `secrets` must stay denied"
    );
    // A collection outside the granted db.read scope is CapabilityRequired.
    assert_eq!(denied.error.unwrap().code(), "CapabilityRequired");

    // The granted collection still works.
    let ok = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(ok.ok, "the granted collection must still be readable after reopen: {:?}", ok.error);
}

/// Review 048 finding 1: the `db.read` scope must NOT be forgeable from the
/// request body. An actor trusted to read only `tasks` cannot reach an ungranted
/// collection by (a) omitting `grants`, or (b) self-expanding `grants` to `["*"]`
/// / the target collection in the payload. The trusted table is the only source
/// of truth; a payload that tries to widen access is rejected, never honored.
#[test]
fn query_execute_db_read_scope_is_not_forgeable_from_payload() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Owner ("dev") is trusted to read ONLY `tasks`.
    core.grant_db_read("dev", ["tasks"]).unwrap();

    // (a) Omitting grants does not fall back to read-all: `secrets` stays denied.
    let omitted = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "secrets" }),
    ));
    assert!(!omitted.ok, "omitting grants must not grant read-all");
    assert_eq!(omitted.error.unwrap().code(), "CapabilityRequired");

    // (b) Self-expanding grants in the payload to the read-all wildcard is an
    // escalation attempt: rejected, not silently honored.
    let widened_star = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "secrets", "grants": {"db": {"read": ["*"]}} }),
    ));
    assert!(!widened_star.ok, "a payload `*` cannot widen the trusted scope");
    assert_eq!(widened_star.error.unwrap().code(), "PermissionDenied");

    // (b') Self-expanding to name the target collection directly is also rejected.
    let widened_named = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "secrets", "grants": {"db": {"read": ["secrets"]}} }),
    ));
    assert!(!widened_named.ok, "a payload self-grant cannot widen the trusted scope");
    assert_eq!(widened_named.error.unwrap().code(), "PermissionDenied");

    // A payload grant that merely RESTATES the trusted scope (a redundant narrow)
    // is harmless: the in-scope `tasks` query still succeeds.
    let restated = core.handle(cmd_as(
        owner(),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks", "grants": {"db": {"read": ["tasks"]}} }),
    ));
    assert!(restated.ok, "a redundant in-scope payload grant must not break the query: {:?}", restated.error);
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
// 6b. explicit seed override on runtime.run (review 032 finding 1): a scenario
//     recorded under fixed seeds can be reproduced THROUGH the facade by pinning
//     `random_seed`/`time_start` in the command payload. The run records exactly
//     those seeds, still replays identically, and a half-specified override is a
//     graceful ValidationError.
// ---------------------------------------------------------------------------

#[test]
fn runtime_run_honors_explicit_seed_override_and_still_replays() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // A pure seam-reading applet so the recorded values depend only on the seeds.
    let seam_ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            const r = await ctx.random.next();
            const t = await ctx.time.now();
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
    install_demo(&mut core, seam_ts, manifest);

    // Run with explicit seeds. The run records EXACTLY them (not the
    // (code_hash,input)-derived defaults), proving the override threads through.
    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {}, "random_seed": 7, "time_start": 500 }),
    ));
    assert!(resp.ok, "seeded run must succeed: {:?}", resp.error);
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();
    let rec = core.store().load_run(&run_id).unwrap().unwrap();
    assert_eq!(rec.random_seed, 7, "explicit random_seed must be recorded");
    assert_eq!(rec.time_start, 500, "explicit time_start must be recorded");
    // The deterministic clock seam starts at the pinned time_start.
    assert_eq!(resp.payload["result"]["value"]["t"], serde_json::json!(500));

    // It still replays byte-identically under the pinned seeds.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(replay.ok, "seeded run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));

    // The host-call method trace is surfaced on the response (facade-asserted).
    assert_eq!(
        resp.payload["host_call_methods"],
        serde_json::json!(["random.next", "time.now"])
    );
}

#[test]
fn runtime_run_rejects_half_specified_seed_override() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    // Only random_seed → ValidationError (a scenario must pin BOTH seams or none).
    let only_random = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {}, "random_seed": 7 }),
    ));
    assert!(!only_random.ok, "a half-specified seed override must be rejected");
    assert_eq!(only_random.error.unwrap().code(), "ValidationError");

    // Only time_start → likewise rejected, and a non-integer seed is rejected too.
    let only_time = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {}, "time_start": 500 }),
    ));
    assert_eq!(only_time.error.unwrap().code(), "ValidationError");
    let bad_type = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {}, "random_seed": "seven", "time_start": 500 }),
    ));
    assert_eq!(bad_type.error.unwrap().code(), "ValidationError");

    // None of the rejected attempts recorded a run.
    assert_eq!(core.events().events_of_kind("run.started").count(), 0);
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

// ---------------------------------------------------------------------------
// 7b. version-pinned replay across a SAME-CODE manifest revision (review 036
//     finding 2): the code_hash-keyed pin is not enough — reinstalling the SAME
//     JS under a different manifest (here, crippled engine `fuel`) used to
//     overwrite program/<code_hash> and strand the old run, which then replayed
//     under the new tighter limits. The PER-RUN pin captures the exact manifest
//     this run used, so the old run still replays identically.
// ---------------------------------------------------------------------------

#[test]
fn replay_uses_recorded_manifest_after_same_code_reinstall_with_tighter_limits() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // An applet that emits a log line (~50 bytes) and renders UI. It completes
    // under a generous `log_bytes` ceiling but trips a tight one — a deterministic,
    // build-independent manifest-sensitive outcome (unlike wall/cpu timing).
    let ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            ctx.log("pinned-manifest replay regression log line padding padding");
            await ctx.ui.render({ type: "Text", text: "pinned" });
            return { ok: true, value: 1 };
        }
    "#;

    // Generous manifest: the run records cleanly under it.
    let generous = demo_manifest();
    install_demo(&mut core, ts, generous);
    let run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "run under the generous manifest must succeed: {:?}", run.error);
    assert_eq!(run.payload["ok"], serde_json::json!(true), "the original run must COMPLETE under the generous manifest");
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();
    let code_hash = run.payload["code_hash"].as_str().unwrap().to_string();

    // Reinstall the SAME JS (so the code_hash is identical) under a manifest whose
    // `log_bytes` ceiling is crippled to 1 — too small for the applet's log line,
    // so a run under THIS manifest deterministically trips ResourceLimitExceeded.
    let mut crippled = demo_manifest();
    crippled["limits"]["log_bytes"] = serde_json::json!(1);
    install_demo(&mut core, ts, crippled);
    // Same code → same code_hash, so the OLD code_hash-keyed pin was overwritten.
    let reinstalled_hash = forge_pipeline::compile(ts).unwrap().code_hash;
    assert_eq!(reinstalled_hash, code_hash, "reinstall is the same code (same code_hash)");

    // A fresh run under the crippled manifest indeed fails on the log-bytes budget
    // — proving the new manifest is genuinely tighter. (The command succeeds; the
    // RUN outcome is the failure, reported in the payload.)
    let crippled_run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(crippled_run.ok, "the run command itself must succeed: {:?}", crippled_run.error);
    assert_eq!(
        crippled_run.payload["ok"],
        serde_json::json!(false),
        "a fresh run under the crippled manifest must FAIL on the resource budget"
    );
    assert_eq!(
        crippled_run.payload["result"]["error"]["kind"],
        serde_json::json!("ResourceLimitExceeded"),
        "the crippled run must fail on the tighter log-bytes budget"
    );

    // Replaying the ORIGINAL run must still succeed and be byte-identical: it
    // replays against the per-run-pinned generous manifest, not the crippled one
    // that overwrote the code_hash-keyed pin.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(
        replay.ok,
        "old run must replay against its pinned (generous) manifest after a same-code tighter reinstall: {:?}",
        replay.error
    );
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// 7c. WRITE-ONCE code_hash fallback (review 038 finding 3): a *legacy* run that
//     has NO per-run pin (recorded before per-run pinning) relies on the
//     content-addressed `program/<code_hash>` fallback. A later same-JS reinstall
//     under a TIGHTER manifest must NOT overwrite that fallback, or the legacy run
//     would replay under the wrong (crippled) limits. We simulate the legacy run
//     by stripping its per-run pin, forcing replay through the fallback, then prove
//     the fallback's generous manifest survived the same-code tighter reinstall.
// ---------------------------------------------------------------------------

#[test]
fn legacy_run_on_codehash_fallback_replays_after_same_code_tighter_reinstall() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Same manifest-sensitive applet as 7b: a ~50-byte log line + a UI render.
    // It completes under a generous `log_bytes` ceiling and trips a tight one.
    let ts = r#"
        export async function main(ctx: any, input: any): Promise<any> {
            ctx.log("write-once fallback replay regression log line padding padding");
            await ctx.ui.render({ type: "Text", text: "fallback" });
            return { ok: true, value: 1 };
        }
    "#;

    // Run under the generous manifest. This pins BOTH the per-run artifact and the
    // code_hash fallback.
    install_demo(&mut core, ts, demo_manifest());
    let run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "generous run must succeed: {:?}", run.error);
    assert_eq!(run.payload["ok"], serde_json::json!(true));
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();

    // Make this a LEGACY run: strip its per-run pin so replay must fall through to
    // the content-addressed `program/<code_hash>` fallback. The key shape
    // (`__forge/meta` namespace, `program/run/<run_id>`) is the stable contract the
    // facade writes via `store_run_program`.
    core.store_mut()
        .kv_delete("__forge/meta", &format!("program/run/{run_id}"))
        .unwrap();

    // Reinstall the SAME JS (identical code_hash) under a crippled `log_bytes`
    // manifest. Pre-fix, this run's `store_program` overwrote the fallback with the
    // crippled manifest, stranding the legacy run. Write-once must preserve the
    // original (generous) fallback.
    let mut crippled = demo_manifest();
    crippled["limits"]["log_bytes"] = serde_json::json!(1);
    install_demo(&mut core, ts, crippled);
    let crippled_run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(crippled_run.ok, "the run command itself must succeed: {:?}", crippled_run.error);
    assert_eq!(
        crippled_run.payload["ok"],
        serde_json::json!(false),
        "the crippled run must FAIL on the tighter log-bytes budget (manifest is genuinely tighter)"
    );

    // The legacy run (now on the fallback only) must still replay byte-identically:
    // the write-once fallback kept its generous manifest, so the log line fits.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(
        replay.ok,
        "legacy fallback run must replay against the write-once (generous) fallback after a same-code tighter reinstall: {:?}",
        replay.error
    );
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// 8. ctx.db.query (DL-15): the applet-facing structured query reaches the real
//    forge-storage engine through the StorageHostBridge, the matched rows flow
//    back into the AppResult, and the call + its rows are RECORDED so replay is
//    byte-identical (the rows are served from the recording, NOT re-run against
//    live storage). The denial case proves an ungranted collection surfaces as
//    the run's CapabilityRequired outcome with no rows (CR-3/SC-10).
// ---------------------------------------------------------------------------

/// An applet that inserts a few task records, then `ctx.db.query`s them with a
/// filter (`priority > input.minPriority`) and an order (`date desc`), returns
/// the matched rows, and renders them. Deterministic: it consumes no seams and
/// writes a fixed record set, so the trace depends only on the (inserted) data.
const QUERY_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const tasks = [
            { title: "Ship spine",    priority: 3, date: "2026-06-10" },
            { title: "Polish docs",   priority: 1, date: "2026-06-12" },
            { title: "Fix replay",    priority: 5, date: "2026-06-11" },
            { title: "Review grants", priority: 4, date: "2026-06-13" }
        ];
        for (const task of tasks) {
            await ctx.db.insert("tasks", task);
        }
        const rows = await ctx.db.query({
            from: "tasks",
            where: ["priority", ">", input.minPriority],
            orderBy: ["date", "desc"]
        });
        const queryRows = rows.map((row: any) => ({
            title: row.title, priority: row.priority, date: row.date
        }));
        await ctx.ui.render({
            type: "Stack", direction: "v",
            children: [
                { type: "Text", text: "Priority Tasks" },
                { type: "List", items: queryRows.map((r: any) => ({ type: "Text", text: r.date + ": " + r.title })) }
            ]
        });
        return { ok: true, value: { count: queryRows.length, query_rows: queryRows } };
    }
"#;

#[test]
fn db_query_filter_order_returns_matched_rows_and_replays_identically() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, QUERY_TS, demo_manifest());

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "minPriority": 2 } }),
    ));
    assert!(resp.ok, "query run must succeed: {:?}", resp.error);
    assert_eq!(resp.payload["ok"], serde_json::json!(true), "run outcome must complete");
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    // The matched rows flowed back into the AppResult: priority > 2 keeps three
    // tasks, ordered by date DESC (newest first).
    let value = &resp.payload["result"]["value"];
    assert_eq!(value["count"], serde_json::json!(3));
    assert_eq!(
        value["query_rows"],
        serde_json::json!([
            { "title": "Review grants", "priority": 4, "date": "2026-06-13" },
            { "title": "Fix replay",    "priority": 5, "date": "2026-06-11" },
            { "title": "Ship spine",    "priority": 3, "date": "2026-06-10" }
        ]),
        "filter (priority > 2) + order (date desc) must shape the rows"
    );

    // The host-call trace shows the four inserts, the db.query, and the render —
    // and db.query was RECORDED as a host call (CR-8).
    assert_eq!(
        resp.payload["host_call_methods"],
        serde_json::json!(["db.insert", "db.insert", "db.insert", "db.insert", "db.query", "ui.render"])
    );
    let saved = core.store().load_run(&run_id).unwrap().unwrap();
    let query_call = saved
        .calls
        .iter()
        .find(|c| c.method == "db.query")
        .expect("the run recorded a db.query call");
    // The recorded response is the exact matched-row array the bridge returned.
    let recorded_rows = query_call.response.as_array().expect("recorded rows are an array");
    assert_eq!(recorded_rows.len(), 3, "the recorded db.query rows are the matched set");
    assert_eq!(recorded_rows[0]["title"], serde_json::json!("Review grants"));

    // All four records actually landed in the projection (the SQLite write link).
    assert_eq!(core.store().list_records("tasks").unwrap().len(), 4);

    // Replay serves the recorded db.query rows (the live bridge is a NullBridge
    // that would error if consulted) and is byte-identical to the original.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(replay.ok, "query run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

/// A `ctx.db.query` over a collection the manifest does NOT grant `db.read` for
/// is denied at host-call time: the policy gate fires (the applet declared no db
/// capability → `CapabilityRequired`, CR-3/SC-10), the live storage is never
/// touched, no rows come back, and the run OUTCOME is the denial. The denial is
/// recorded, so the run still replays byte-identically.
#[test]
fn db_query_on_ungranted_collection_is_denied_with_no_rows() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Manifest grants ui but NO db capability at all (empty read+write), so a
    // db.read on any collection is CapabilityRequired (the category is undeclared).
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    let denied_ts = r#"
        export async function main(ctx: any, _input: any): Promise<any> {
            const rows = await ctx.db.query({ from: "secrets", limit: 1 });
            return { ok: true, value: { query_rows: rows } };
        }
    "#;
    install_demo(&mut core, denied_ts, manifest);

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    // The run COMMAND completes (it records the failure); the run OUTCOME is the
    // capability denial.
    assert!(resp.ok, "the run command itself completes (records the denial)");
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "run outcome must be the denial");
    assert_eq!(
        resp.payload["result"]["error"]["kind"],
        serde_json::json!("CapabilityRequired"),
        "an ungranted db.read query is CapabilityRequired (undeclared db category): {}",
        resp.payload["result"]
    );

    // Only the denied db.query was attempted (no rows, no further effect).
    assert_eq!(resp.payload["host_call_methods"], serde_json::json!(["db.query"]));
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    // The denial was recorded → the run replays byte-identically.
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(replay.ok, "the denied run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// 9. CRDT-backed record writes through the spine (DL-4) + projection rebuild
//    (DL-6): a `ctx.db.insert` in a run is no longer a projection-only write.
//    It becomes a Loro op on the collection's RecordsDoc whose incremental update
//    is appended to `crdt_chunks` (+ an oplog row) AND materializes the `records`
//    projection row — all in one SQLite transaction. The CRDT docs are the source
//    of truth, so dropping and rebuilding the projection purely from the chunks
//    (`Store::rebuild_projection`) reproduces the SAME record. Observable behavior
//    is unchanged: the record is still queryable/returned and the run still
//    replays byte-identically.
// ---------------------------------------------------------------------------

#[test]
fn db_insert_through_spine_writes_crdt_chunk_oplog_and_rebuildable_projection() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Buy milk" } }),
    ));
    assert!(resp.ok, "run must succeed: {:?}", resp.error);
    assert_eq!(resp.payload["ok"], serde_json::json!(true));
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    // (a) The record landed in the `records` PROJECTION (still queryable/returned).
    let rec = core
        .store()
        .get_record("tasks", "tasks/1")
        .unwrap()
        .expect("ctx.db.insert must materialize the projection row");
    assert_eq!(rec.fields["title"], serde_json::json!("Buy milk"));
    assert_eq!(rec.fields["done"], serde_json::json!(false));

    // (b) A CRDT CHUNK was written for the collection doc (DL-4): the insert is a
    //     Loro op whose incremental update is appended to `crdt_chunks`. The doc id
    //     is the collection doc selector `collection/tasks`.
    let doc_id = forge_storage::collection_doc_id("tasks");
    let chunks = core.store().get_chunks(&doc_id).unwrap();
    assert_eq!(
        chunks.len(),
        1,
        "exactly one CRDT chunk for the single insert (DL-4)"
    );
    assert_eq!(chunks[0].chunk_id, "chunk-0001", "first chunk id is immutable + sequenced");
    assert_eq!(chunks[0].format, forge_storage::CHUNK_FORMAT);

    // (c) An OPLOG row was appended for the same write (DL-4 write metadata), of
    //     the logical kind `record.insert`.
    let ops = core.store().list_ops().unwrap();
    assert_eq!(ops.len(), 1, "one oplog row for the one insert");
    assert_eq!(ops[0].kind, "record.insert");

    // (d) DL-6: drop the ENTIRE projection and rebuild it purely from the persisted
    //     `crdt_chunks` (the CRDT docs are the source of truth). The rebuilt
    //     projection must reproduce the SAME record — proving the projection is a
    //     derived, rebuildable view of the CRDT op log, not the source of truth.
    let before = core.store().get_record("tasks", "tasks/1").unwrap().unwrap();
    let idx = forge_storage::IndexManager::new();
    core.store_mut().rebuild_projection(&idx).unwrap();
    let after = core
        .store()
        .get_record("tasks", "tasks/1")
        .unwrap()
        .expect("rebuild-from-chunks must reproduce the record (DL-6)");
    assert_eq!(after, before, "rebuilt projection record must equal the maintained one");
    assert_eq!(after.fields["title"], serde_json::json!("Buy milk"));
    // The chunk history survives the rebuild (append-only), and no extra rows leak.
    assert_eq!(core.store().get_chunks(&doc_id).unwrap().len(), 1);
    assert_eq!(core.store().list_records("tasks").unwrap().len(), 1);

    // (e) Observable behavior is unchanged: the run still replays byte-identically
    //     (the RunRecord trace records the same db.insert ack — `tasks/1`).
    let saved = core.store().load_run(&run_id).unwrap().unwrap();
    let insert_call = saved
        .calls
        .iter()
        .find(|c| c.method == "db.insert")
        .expect("the run recorded a db.insert");
    assert_eq!(
        insert_call.response,
        serde_json::json!("tasks/1"),
        "the recorded db.insert ack is the returned id, unchanged by the CRDT path"
    );
    let replay = core.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_id })));
    assert!(replay.ok, "the CRDT-backed run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// 10. workspace.export / workspace.import (DL-24): a workspace exports as a
//     single portable file; re-importing into a FRESH workspace reproduces the
//     records byte-for-byte AND carries the applet (manifest + compiled program)
//     so the imported workspace can RUN it. The DL-24 portable-workspace promise,
//     end to end through the command facade.
// ---------------------------------------------------------------------------

#[test]
fn export_import_round_trips_records_and_the_runnable_applet() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");

    // --- Workspace A: install the demo applet + run it (writes a record). ----
    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    let run_a = a.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Portable note" } }),
    ));
    assert!(run_a.ok, "source run must succeed: {:?}", run_a.error);
    assert_eq!(a.store().list_records("tasks").unwrap().len(), 1);
    let src_record = a.store().get_record("tasks", "tasks/1").unwrap().unwrap();

    // --- Export A to the portable single-file bundle. ------------------------
    let export = a.handle(cmd(
        "workspace.export",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(export.ok, "export must succeed: {:?}", export.error);
    assert_eq!(export.payload["export_format_version"], serde_json::json!(1));
    // The report names what travelled (the applet manifest+program) and that
    // run logs were excluded by default (privacy).
    assert_eq!(
        export.payload["included"]["applet_manifests_and_programs"],
        serde_json::json!(1),
        "the applet manifest + program travel so the import can run it"
    );
    assert_eq!(export.payload["include_run_logs"], serde_json::json!(false));
    assert!(bundle.exists(), "the bundle file was written");

    // --- Import into a FRESH workspace B (the typed constructor API). --------
    let mut b = WorkspaceCore::import_from_file(&bundle, "ws_b").unwrap();

    // (a) B has the SAME record (re-derived from the imported CRDT chunks, DL-6).
    let dst_record = b
        .store()
        .get_record("tasks", "tasks/1")
        .unwrap()
        .expect("imported workspace must have the source record");
    assert_eq!(dst_record, src_record, "DL-24: the record round-trips byte-for-byte");
    // Queryable through the facade too.
    let q = b.handle(cmd("query.execute", None, serde_json::json!({ "collection": "tasks" })));
    assert!(q.ok, "{:?}", q.error);
    let rows = q.payload["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fields"]["title"], serde_json::json!("Portable note"));

    // (b) B can RUN the imported applet (its manifest + compiled program travelled
    //     in the portable __forge/meta namespace) — a fresh insert lands a 2nd row.
    let run_b = b.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "Second note" } }),
    ));
    assert!(run_b.ok, "the imported applet must run in B: {:?}", run_b.error);
    assert_eq!(run_b.payload["ok"], serde_json::json!(true), "imported applet run completes");
    assert_eq!(
        b.store().list_records("tasks").unwrap().len(),
        2,
        "the imported applet wrote a new record into B"
    );

    // (c) The run recorded in B replays byte-identically (the imported applet's
    //     code_hash provenance survived the round-trip).
    let run_b_id = run_b.payload["run_id"].as_str().unwrap().to_string();
    let replay_b = b.handle(cmd("runtime.replay", None, serde_json::json!({ "run_id": run_b_id })));
    assert!(replay_b.ok, "imported-applet run must replay: {:?}", replay_b.error);
    assert_eq!(replay_b.payload["replays_identically"], serde_json::json!(true));
}

/// The `workspace.import` COMMAND imports a bundle into THIS fresh workspace in
/// place (the facade path, distinct from the `import_from_file` constructor),
/// reports what was reconstructed, and refuses to import over a populated
/// workspace.
#[test]
fn workspace_import_command_loads_into_this_fresh_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");

    // Source: a run that writes one record, then export.
    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "X" } })));
    let export = a.handle(cmd("workspace.export", None, serde_json::json!({ "path": bundle.to_str().unwrap() })));
    assert!(export.ok, "{:?}", export.error);

    // Import into a fresh workspace via the COMMAND (in place).
    let mut b = WorkspaceCore::in_memory("ws_b").unwrap();
    let import = b.handle(cmd("workspace.import", None, serde_json::json!({ "path": bundle.to_str().unwrap() })));
    assert!(import.ok, "import command must succeed: {:?}", import.error);
    assert_eq!(import.payload["records"], serde_json::json!(1), "one record reconstructed");
    assert_eq!(import.payload["collections"], serde_json::json!(["tasks"]));
    assert_eq!(import.payload["imported_applets"], serde_json::json!(["applet/app_demo"]));
    // A workspace.imported event was emitted.
    assert_eq!(b.events().events_of_kind("workspace.imported").count(), 1);

    // The record is present + queryable in B.
    assert_eq!(b.store().list_records("tasks").unwrap().len(), 1);

    // Re-importing into the NOW-populated workspace is refused (no silent merge).
    let again = b.handle(cmd("workspace.import", None, serde_json::json!({ "path": bundle.to_str().unwrap() })));
    assert!(!again.ok, "import must refuse a populated workspace");
    assert_eq!(again.error.unwrap().code(), "ValidationError");
}

/// DL-24 deterministic export: two exports of the same workspace produce
/// byte-identical bundle files (stable table + row ordering through the facade).
#[test]
fn workspace_export_is_byte_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "Det" } })));

    let p1 = dir.path().join("a.forgews");
    let p2 = dir.path().join("b.forgews");
    assert!(a.handle(cmd("workspace.export", None, serde_json::json!({ "path": p1.to_str().unwrap() }))).ok);
    assert!(a.handle(cmd("workspace.export", None, serde_json::json!({ "path": p2.to_str().unwrap() }))).ok);
    assert_eq!(
        std::fs::read(&p1).unwrap(),
        std::fs::read(&p2).unwrap(),
        "two exports of the same workspace must be byte-identical"
    );
}

/// DL-24 run-log policy: run logs (the `runs` table) are EXCLUDED by default and
/// INCLUDED only with `include_run_logs: true` (a debug/backup bundle).
#[test]
fn export_run_logs_are_excluded_by_default_included_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    let run = a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "Logged" } })));
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();

    // Default export: runs excluded → the imported workspace cannot load the run.
    let excluded_bundle = dir.path().join("excluded.forgews");
    assert!(a.handle(cmd("workspace.export", None, serde_json::json!({ "path": excluded_bundle.to_str().unwrap() }))).ok);
    let b = WorkspaceCore::import_from_file(&excluded_bundle, "ws_b").unwrap();
    assert!(b.store().load_run(&run_id).unwrap().is_none(), "runs excluded by default");

    // Debug bundle: include_run_logs → the run round-trips.
    let included_bundle = dir.path().join("included.forgews");
    let export = a.handle(cmd(
        "workspace.export",
        None,
        serde_json::json!({ "path": included_bundle.to_str().unwrap(), "include_run_logs": true }),
    ));
    assert!(export.ok, "{:?}", export.error);
    assert_eq!(export.payload["include_run_logs"], serde_json::json!(true));
    let c = WorkspaceCore::import_from_file(&included_bundle, "ws_c").unwrap();
    assert_eq!(
        c.store().load_run(&run_id).unwrap().expect("run must round-trip in a debug bundle").run_id.as_str(),
        run_id
    );
}

/// DL-24 exclusion guard at the facade: a secret KV value (a `secret/` namespace)
/// is NEVER written to the export bundle, so it never reaches the imported
/// workspace. Local-only/secret data does not travel with the portable file.
#[test]
fn export_never_carries_secret_kv_to_the_imported_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");

    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "S" } })));
    // Plant a secret directly in the store (a provider token / api key bucket).
    a.store_mut()
        .kv_set("secret/weather", "api_key", b"sk-DO-NOT-EXPORT", "text/plain")
        .unwrap();
    // And a portable applet ctx.storage value that SHOULD travel, as a control.
    a.store_mut()
        .kv_set("applet/app_demo", "draft", b"keep-me", "text/plain")
        .unwrap();

    assert!(a.handle(cmd("workspace.export", None, serde_json::json!({ "path": bundle.to_str().unwrap() }))).ok);
    let b = WorkspaceCore::import_from_file(&bundle, "ws_b").unwrap();

    // The secret did NOT travel; the portable applet value DID.
    assert_eq!(b.store().kv_get("secret/weather", "api_key").unwrap(), None, "secrets are never exported");
    assert_eq!(
        b.store().kv_get("applet/app_demo", "draft").unwrap().as_deref(),
        Some(&b"keep-me"[..]),
        "portable applet ctx.storage travels with the bundle"
    );
}

/// Command-level RBAC (CR-A3, forge/spec/commands.md): export is Owner /
/// Maintainer / Auditor; import is Owner only. A read-only Viewer cannot export,
/// and a non-Owner cannot import.
#[test]
fn export_import_rbac_is_gated_per_commands_spec() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");

    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "R" } })));

    // A Viewer cannot export (not in Owner/Maintainer/Auditor).
    let viewer_export = a.handle(cmd_as(
        actor(Role::Viewer),
        "workspace.export",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(!viewer_export.ok, "Viewer must not export");
    assert_eq!(viewer_export.error.unwrap().code(), "PermissionDenied");
    assert!(!bundle.exists(), "a denied export writes no bundle");

    // An Auditor CAN export (a backup/debug bundle).
    let auditor_export = a.handle(cmd_as(
        actor(Role::Auditor),
        "workspace.export",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(auditor_export.ok, "Auditor must be permitted to export: {:?}", auditor_export.error);

    // A Maintainer cannot import (import is Owner-only per commands.md).
    let mut b = WorkspaceCore::in_memory("ws_b").unwrap();
    let maint_import = b.handle(cmd_as(
        actor(Role::Maintainer),
        "workspace.import",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(!maint_import.ok, "import is Owner-only");
    assert_eq!(maint_import.error.unwrap().code(), "PermissionDenied");

    // The Owner can import.
    let owner_import = b.handle(cmd(
        "workspace.import",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(owner_import.ok, "Owner must be permitted to import: {:?}", owner_import.error);
}

/// Review 062 P1 #1: `workspace.import` into a FILE-BACKED workspace must PERSIST
/// to the target file. The import is committed to the SAME SQLite file the
/// workspace already holds, so after dropping the handle and reopening the same
/// path the imported applet (manifest + compiled program), record, and the
/// portable `db.read` grant table are all still present. The pre-fix code
/// imported into a separate in-memory store and swapped it in, so a reopen of the
/// original file saw an empty workspace — import reported success but lost
/// everything on exit.
#[test]
fn workspace_import_persists_into_a_file_backed_workspace_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");
    let target = dir.path().join("imported.forge");

    // --- Source A (file-backed): install + run (a record), provision a grant,
    //     then export the portable bundle. -----------------------------------
    let src_path = dir.path().join("source.forge");
    {
        let mut a = WorkspaceCore::open(&src_path, "ws_a").unwrap();
        install_demo(&mut a, DEMO_TS, demo_manifest());
        let run = a.handle(cmd(
            "runtime.run",
            Some("app_demo"),
            serde_json::json!({ "input": { "title": "Persisted note" } }),
        ));
        assert!(run.ok, "source run must succeed: {:?}", run.error);
        // A portable workspace-config grant that must survive the round-trip.
        a.grant_db_read("auditor", ["tasks"]).unwrap();
        let export = a.handle(cmd(
            "workspace.export",
            None,
            serde_json::json!({ "path": bundle.to_str().unwrap() }),
        ));
        assert!(export.ok, "export must succeed: {:?}", export.error);
    }

    // --- Import the bundle into a FRESH FILE-BACKED workspace B, then DROP B. -
    {
        let mut b = WorkspaceCore::open(&target, "ws_b").unwrap();
        let import = b.handle(cmd(
            "workspace.import",
            None,
            serde_json::json!({ "path": bundle.to_str().unwrap() }),
        ));
        assert!(import.ok, "file-backed import must succeed: {:?}", import.error);
        assert_eq!(import.payload["records"], serde_json::json!(1), "one record reconstructed");
        assert_eq!(import.payload["imported_applets"], serde_json::json!(["applet/app_demo"]));
        // Visible in this handle before drop.
        assert_eq!(b.store().list_records("tasks").unwrap().len(), 1);
        // The dropped handle releases the SQLite connection on the target file.
    }

    // --- REOPEN the SAME path: every imported piece survived to disk. --------
    let mut reopened = WorkspaceCore::open(&target, "ws_b").unwrap();

    // (a) The imported RECORD persisted (re-derived from the imported CRDT chunks).
    let rec = reopened
        .store()
        .get_record("tasks", "tasks/1")
        .unwrap()
        .expect("the imported record must survive drop + reopen of the file");
    assert_eq!(rec.fields["title"], serde_json::json!("Persisted note"));
    assert_eq!(reopened.store().list_records("tasks").unwrap().len(), 1);

    // (b) The imported APPLET (manifest + compiled program) persisted: the
    //     reopened workspace can RUN it, writing a 2nd record.
    let run_b = reopened.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": "After reopen" } }),
    ));
    assert!(run_b.ok, "the imported applet must run after reopen: {:?}", run_b.error);
    assert_eq!(
        reopened.store().list_records("tasks").unwrap().len(),
        2,
        "the persisted applet wrote a new record after reopen"
    );

    // (c) The portable `db.read` GRANT table persisted: the scoped `auditor` is
    //     still confined to `tasks` after the import was written to disk and
    //     reopened (an ungranted collection stays denied, no fail-open).
    let denied = reopened.handle(cmd_as(
        actor(Role::Auditor),
        "query.execute",
        None,
        serde_json::json!({ "collection": "secrets" }),
    ));
    assert!(
        !denied.ok,
        "the imported db.read grant must persist: `secrets` stays denied for the scoped auditor"
    );
    assert_eq!(denied.error.unwrap().code(), "CapabilityRequired");
    let granted = reopened.handle(cmd_as(
        actor(Role::Auditor),
        "query.execute",
        None,
        serde_json::json!({ "collection": "tasks" }),
    ));
    assert!(granted.ok, "the granted collection stays readable after reopen: {:?}", granted.error);
}

/// Review 062 P1 #2: the fresh-target precondition uses the storage-level
/// `is_empty_target` (every importable table/namespace), so a workspace that is
/// "empty" only in its records projection but already carries portable state —
/// here a `db.read` GRANTS-only workspace — is correctly NOT fresh and the import
/// is refused. The pre-fix records/applet/oplog-only check let a grants-only
/// target pass and silently shadowed the existing grant table.
#[test]
fn workspace_import_refuses_a_grants_only_non_empty_target() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("ws.forgews");

    // Source A: a run + export so we have a real bundle to attempt to import.
    let mut a = WorkspaceCore::in_memory("ws_a").unwrap();
    install_demo(&mut a, DEMO_TS, demo_manifest());
    a.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": { "title": "X" } })));
    assert!(a
        .handle(cmd("workspace.export", None, serde_json::json!({ "path": bundle.to_str().unwrap() })))
        .ok);

    // Target B holds ONLY a db.read grant (no records, no applet meta, no oplog):
    // empty by the OLD check, but it carries portable kv state.
    let mut b = WorkspaceCore::in_memory("ws_b").unwrap();
    b.grant_db_read("dev", ["tasks"]).unwrap();

    let import = b.handle(cmd(
        "workspace.import",
        None,
        serde_json::json!({ "path": bundle.to_str().unwrap() }),
    ));
    assert!(!import.ok, "import must refuse a grants-only (non-empty) target, not silently overwrite it");
    assert_eq!(import.error.unwrap().code(), "ValidationError");

    // And the pre-existing grant is untouched (the refused import wrote nothing).
    assert!(
        b.store().list_records("tasks").unwrap().is_empty(),
        "a refused import must not populate the target"
    );
}

// ---------------------------------------------------------------------------
// 9. ctx.net.fetch (prd-merged/07 SC-5/SC-8, prd-merged/01 CR-3 net namespace):
//    the applet-facing network egress capability reaches the StorageHostBridge's
//    INJECTED HttpClient — but only AFTER the runtime's HostContext has run the
//    SC-5 NetPolicy against the applet's manifest `net` allowlist, and the
//    response is RECORDED so replay is byte-identical (CR-8: no live network on
//    replay; the recording is served). The actual HTTP is behind an injectable
//    trait, so these tests inject a MOCK client and NEVER touch the live network.
//    The denied case proves an ungranted domain surfaces as the run's
//    CapabilityRequired outcome with the mock never called.
// ---------------------------------------------------------------------------

use forge_runtime::{HttpClient, NetRequest, NetResponse};
use std::sync::{Arc, Mutex};

/// A network-free [`HttpClient`] test double that (a) records every request it
/// receives (so a test can assert the *policy-approved* request reached it) and
/// (b) returns a canned JSON response. NO live network: this is exactly the
/// injectable seam that keeps CI/the demo offline.
#[derive(Clone)]
struct RecordingMockClient {
    seen: Arc<Mutex<Vec<NetRequest>>>,
    response: NetResponse,
}

impl RecordingMockClient {
    fn new(response: NetResponse) -> Self {
        RecordingMockClient { seen: Arc::new(Mutex::new(Vec::new())), response }
    }
}

impl HttpClient for RecordingMockClient {
    fn send(&self, request: NetRequest) -> forge_domain::Result<NetResponse> {
        self.seen.lock().unwrap().push(request);
        Ok(self.response.clone())
    }
}

/// Manifest granting `net` egress to `https://api.example.com/*` (GET), plus ui.
/// No db/storage grants — this applet only fetches and renders.
fn net_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true,
            "net": [
                { "method": "GET", "url": "https://api.example.com/*",
                  "response_content_types": ["application/json"] }
            ]
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    })
}

/// An applet that `ctx.net.fetch`-es a weather endpoint and renders the parsed
/// body. Deterministic: it reads no seams; its trace depends only on the recorded
/// fetch response.
const NET_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const resp = await ctx.net.fetch({
            method: "GET",
            url: "https://api.example.com/weather"
        });
        const parsed = JSON.parse(resp.body);
        await ctx.ui.render({
            type: "Stack", direction: "v",
            children: [
                { type: "Text", text: "Weather" },
                { type: "Text", text: "temp: " + parsed.temp }
            ]
        });
        return { ok: true, value: { status: resp.status, temp: parsed.temp } };
    }
"#;

#[test]
fn net_fetch_runs_through_injected_client_records_and_replays_identically() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Inject a recording mock as the network seam (the host/shell injection
    // point). The factory builds a fresh client per run; we keep a handle to the
    // shared `seen` log so we can assert the policy-approved request reached it.
    let canned = NetResponse {
        status: 200,
        body: Some(r#"{"temp":21}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };
    let mock = RecordingMockClient::new(canned);
    let seen = mock.seen.clone();
    core.set_http_client_factory(move || Box::new(mock.clone()));

    install_demo(&mut core, NET_TS, net_manifest());
    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    assert!(resp.ok, "net run must succeed: {:?}", resp.error);
    assert_eq!(resp.payload["ok"], serde_json::json!(true), "run outcome must complete");
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    // The mock was called EXACTLY ONCE, with the policy-approved request (the
    // host/scheme/path the manifest allowlisted, method GET).
    let approved = seen.lock().unwrap();
    assert_eq!(approved.len(), 1, "exactly one allowed fetch reached the client");
    assert_eq!(approved[0].method, "GET");
    assert_eq!(approved[0].url, "https://api.example.com/weather");
    drop(approved);

    // The parsed body flowed back into the AppResult.
    assert_eq!(resp.payload["result"]["value"]["status"], serde_json::json!(200));
    assert_eq!(resp.payload["result"]["value"]["temp"], serde_json::json!(21));

    // The fetch is in the recorded host-call trace (CR-8).
    assert!(
        resp.payload["host_call_methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "net.fetch"),
        "the run records net.fetch: {}",
        resp.payload["host_call_methods"]
    );

    // The RunRecord captured the net.fetch response.
    let rec = core.store().load_run(&run_id).unwrap().unwrap();
    let net_call = rec
        .calls
        .iter()
        .find(|c| c.method == "net.fetch")
        .expect("net.fetch must be recorded");
    assert_eq!(net_call.response["status"], serde_json::json!(200));
    assert_eq!(net_call.response["body"], serde_json::json!(r#"{"temp":21}"#));

    // Replay is byte-identical AND serves the RECORDED response — it never touches
    // the live client (the replay path uses a NullBridge; no live network, CR-8).
    let before = seen.lock().unwrap().len();
    let replay = core.handle(cmd(
        "runtime.replay",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(replay.ok, "net run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "replay must serve the recorded response, NOT re-call the live client"
    );
}

#[test]
fn net_fetch_to_ungranted_domain_is_denied_and_never_reaches_the_client() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Inject a recording mock; a denied fetch must NEVER reach it.
    let mock = RecordingMockClient::new(NetResponse {
        status: 200,
        body: Some(r#"{"temp":21}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    });
    let seen = mock.seen.clone();
    core.set_http_client_factory(move || Box::new(mock.clone()));

    // Manifest with NO net grant at all → the egress policy maps the fetch to
    // CapabilityRequired (the applet never requested the `net` capability).
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    install_demo(&mut core, NET_TS, manifest);

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    // The run command completes (the denial is recorded), but the RUN outcome is a
    // failure: the applet's ctx.net.fetch threw the policy denial.
    assert!(resp.ok, "the run command itself completes (records the denial)");
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "run outcome must be a denial");
    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("CapabilityRequired"),
        "an absent net grant surfaces CapabilityRequired: {result_str}"
    );

    // The mock was NEVER called — the egress policy denied the fetch before it
    // could reach the injected client.
    assert!(
        seen.lock().unwrap().is_empty(),
        "a denied fetch must not reach the injected HttpClient"
    );
    assert_eq!(core.events().events_of_kind("run.failed").count(), 1);
}

#[test]
fn net_fetch_to_non_allowlisted_path_is_permission_denied() {
    // A non-empty net grant that does NOT cover the requested host → the egress
    // policy denies with PermissionDenied (distinct from the empty-grant
    // CapabilityRequired case), and the mock is never called.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let mock = RecordingMockClient::new(NetResponse {
        status: 200,
        body: Some(r#"{"temp":21}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    });
    let seen = mock.seen.clone();
    core.set_http_client_factory(move || Box::new(mock.clone()));

    // Grants net to a DIFFERENT host than the applet fetches.
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true,
            "net": [ { "method": "GET", "url": "https://other.example.com/*" } ]
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    install_demo(&mut core, NET_TS, manifest);

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    assert!(resp.ok, "the run command itself completes (records the denial)");
    assert_eq!(resp.payload["ok"], serde_json::json!(false));
    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("PermissionDenied"),
        "a non-matching net rule surfaces PermissionDenied: {result_str}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "a denied fetch must not reach the injected HttpClient"
    );
}

#[test]
fn net_fetch_with_no_injected_client_fails_closed_platform_unavailable() {
    // The DEFAULT (no `set_http_client_factory`) is the fail-closed NoNetworkClient:
    // an allowed fetch with no client wired surfaces PlatformUnavailable rather than
    // reaching the network. This is the CI/demo posture (no client is ever injected
    // there). The fetch is policy-ALLOWED here (the manifest grants the host), so the
    // denial proves the *bridge*'s default client refused, not the egress policy.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, NET_TS, net_manifest());

    let resp = core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": {} }),
    ));
    assert!(resp.ok, "the run command itself completes");
    assert_eq!(resp.payload["ok"], serde_json::json!(false), "no client wired → run fails");
    let result_str = resp.payload["result"].to_string();
    assert!(
        result_str.contains("PlatformUnavailable")
            && result_str.contains("no network client configured"),
        "the default bridge client refuses with PlatformUnavailable: {result_str}"
    );
}

// ---------------------------------------------------------------------------
// SC-15 / MP-4 — app signing/trust at install (signing-ready, M0a).
//
// `applet.install` MAY carry an optional Ed25519-signed package under a
// `signature` field; when present the platform VERIFIES it (forge-signing) over
// the canonical `terrane/sig/v1` preimage BEFORE trusting/installing:
//
//   - a SIGNED package that verifies installs OK + records the trust (publisher);
//   - a TAMPERED signed package is REJECTED with ValidationError + nothing stored;
//   - an install with NO signature proceeds unsigned (the response says so).
//
// These drive the committed T012 vectors in forge/fixtures/signing/ through the
// real facade, so the core's preimage/verify wiring is proven against the exact
// bytes the fixtures signed.
// ---------------------------------------------------------------------------

/// Load a signing fixture (`forge/fixtures/signing/<name>`) as JSON.
fn load_signing_fixture(name: &str) -> serde_json::Value {
    // CARGO_MANIFEST_DIR = forge/crates/core
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/signing")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read signing fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse signing fixture {name}: {e}"))
}

/// Build the `applet.install` `signature` payload block from a T012 fixture: the
/// signed package + signature + public key (the fixtures carry `public_key_pem`),
/// and — when the fixture exercises the policy layer — its publisher-trust block
/// mapped to the verifier's `{publisher, trusted, valid_until}` shape
/// (`status == "unknown"` → not trusted).
fn signature_block_from_fixture(fixture: &serde_json::Value) -> serde_json::Value {
    let mut block = serde_json::json!({
        "package": fixture["package"].clone(),
        "signature": fixture["signature"].clone(),
        "public_key": fixture["public_key_pem"].clone(),
    });
    if let Some(trust) = fixture.get("publisher_trust") {
        let trusted = trust.get("status").and_then(|s| s.as_str()) != Some("unknown");
        block["publisher_trust"] = serde_json::json!({
            "publisher": trust["publisher"].clone(),
            "trusted": trusted,
            "valid_until": trust.get("valid_until").cloned().unwrap_or(serde_json::Value::Null),
        });
    }
    block
}

/// Build the install `sources` map FROM a signed fixture's `package.files`, so
/// the install carries exactly the code the signature signed. The signature is
/// bound to the install sources (review 080 #1), so a signed install must ship
/// the signed files — these fixture sources are valid TypeScript the pipeline
/// compiles.
fn sources_from_fixture(fixture: &serde_json::Value) -> serde_json::Value {
    let mut sources = serde_json::Map::new();
    for file in fixture["package"]["files"].as_array().expect("files array") {
        sources.insert(
            file["path"].as_str().expect("file path").to_string(),
            file["content"].clone(),
        );
    }
    serde_json::Value::Object(sources)
}

/// The signed `appId` carried in `valid_signature.json`'s package manifest. A
/// signed install MUST be installed under THIS local applet id — review 083 #1
/// binds the requested `applet_id` to the signed `appId`, so a valid signature
/// for one app identity cannot bless a different local id. Every positive
/// signed-install test installs under this id.
const SIGNED_APP_ID: &str = "app.notes";

/// The forge-domain manifest that MATCHES the `valid_signature.json` signed
/// package manifest's capability boundary + resource limits (review 082 #1): the
/// signed `app.notes` package grants `storage notes/*`, `db notes`, `ui`, no
/// network, and a `{wall_ms: 3000, memory_bytes: 67108864}` budget. A signed
/// install must enforce EXACTLY this boundary, so the positive signed-install
/// tests ship this manifest (not `demo_manifest()`, whose broader `storage app/*`
/// / `db tasks` grants the signed package never blessed — the gap review 082
/// exposed). The four limits the signed manifest does not declare keep the M0a
/// defaults; only `wall_ms`/`memory_bytes` are bound.
fn signed_fixture_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": ["notes/*"], "write": ["notes/*"] },
            "db": { "read": ["notes"], "write": ["notes"] },
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

/// Install an applet WITH an attached signature block (from a fixture), shipping
/// the SIGNED files as the install sources AND a manifest that matches the signed
/// package's capability boundary, so both binds (sources — review 080 #1, and
/// manifest/policy — review 082 #1) are satisfied and the install records
/// `Signed`.
fn install_demo_signed(
    core: &mut WorkspaceCore,
    applet_id: &str,
    fixture: &serde_json::Value,
    signature: serde_json::Value,
) -> forge_domain::CoreResponse {
    core.handle(cmd(
        "applet.install",
        Some(applet_id),
        serde_json::json!({
            "manifest": signed_fixture_manifest(),
            "sources": sources_from_fixture(fixture),
            "signature": signature,
        }),
    ))
}

#[test]
fn install_signed_package_verifies_and_records_trust() {
    // A valid T012 signed package: the install verifies the Ed25519 signature
    // over the canonical preimage and records the verified publisher as trust.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let sig = signature_block_from_fixture(&fixture);
    // review 083 #1: install under the SIGNED appId, not a different local id —
    // the appId bind requires `applet_id == package.manifest.appId`.
    let resp = install_demo_signed(&mut core, SIGNED_APP_ID, &fixture, sig);

    assert!(resp.ok, "a valid signed package must install: {:?}", resp.error);
    let trust = &resp.payload["trust"];
    assert_eq!(trust["status"], serde_json::json!("signed"), "trust recorded as signed: {trust}");
    assert_eq!(
        trust["publisher"],
        serde_json::json!("test-publisher"),
        "the verified publisher is recorded for later trust reporting"
    );
    assert_eq!(
        trust["key_id"],
        serde_json::json!("test-ed25519-2026-06"),
        "the signing key id is recorded"
    );

    // The trust is also surfaced on the applet.installed event.
    let installed_evt = core
        .events()
        .events_of_kind("applet.installed")
        .next()
        .expect("applet.installed emitted");
    assert_eq!(installed_evt.payload["trust"]["status"], serde_json::json!("signed"));

    // And the signed applet actually runs (the install was real, not just a check).
    let run = core.handle(cmd("runtime.run", Some(SIGNED_APP_ID), serde_json::json!({ "input": {} })));
    assert!(run.ok, "the verified applet runs: {:?}", run.error);
}

#[test]
fn a_signature_cannot_bless_a_broader_top_level_manifest() {
    // review 082 #1: a valid T012 signed package whose CODE is identical to the
    // install sources, but whose top-level `manifest` grants BROADER capabilities
    // than the signed package manifest, must be REJECTED — not installed as
    // `Signed` under a policy the publisher never blessed. The signed
    // `valid_signature` package grants `storage notes/*` + `db notes`; here the
    // install ships the broader `demo_manifest()` (`storage app/*` + `db tasks`).
    // The sources bind passes (identical files), so this exercises the NEW
    // manifest/policy bind, which rejects before anything is stored.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let resp = core.handle(cmd(
        "applet.install",
        // Install under the SIGNED appId so the appId bind passes and this test
        // isolates the CAPABILITIES mismatch (review 083 #1 binds appId first).
        Some(SIGNED_APP_ID),
        serde_json::json!({
            // Identical code to the signed package, but a broader manifest.
            "manifest": demo_manifest(),
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));

    assert!(
        !resp.ok,
        "a signed install whose manifest grants more than the signed package must be rejected"
    );
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string()
            .contains("install manifest does not match the signed package manifest"),
        "the manifest/policy mismatch is surfaced: {err}"
    );

    // Nothing was installed: the broader policy never reached the store.
    let run = core.handle(cmd("runtime.run", Some(SIGNED_APP_ID), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "the rejected install stored nothing");

    // And the SAME signed package WITH a matching manifest still installs as
    // Signed — proving the bind rejects only the mismatch, not signed installs.
    let mut core_ok = WorkspaceCore::in_memory("ws1").unwrap();
    let ok = install_demo_signed(
        &mut core_ok,
        SIGNED_APP_ID,
        &fixture,
        signature_block_from_fixture(&fixture),
    );
    assert!(ok.ok, "a matching signed install still succeeds: {:?}", ok.error);
    assert_eq!(ok.payload["trust"]["status"], serde_json::json!("signed"));
}

#[test]
fn a_signature_cannot_bless_different_resource_limits() {
    // review 082 #1, limits dimension: identical code, identical capabilities,
    // but a top-level `wall_ms` that differs from the signed package's
    // `resourceBudget.wall_ms` (3000) must be rejected — a publisher's signed
    // resource boundary cannot be widened at install.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let mut manifest = signed_fixture_manifest();
    manifest["limits"]["wall_ms"] = serde_json::json!(60000); // signed is 3000

    let resp = core.handle(cmd(
        "applet.install",
        Some(SIGNED_APP_ID),
        serde_json::json!({
            "manifest": manifest,
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));

    assert!(!resp.ok, "a wider resource budget than the signed one must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("limits.wall_ms"),
        "the limits mismatch is surfaced: {err}"
    );
}

// --- review 083 regression tests: each fails if the corresponding bind is
// reverted. Together they pin the FULL signed-policy surface (appId, every
// resource limit, the whole net rule, the entrypoint), plus that a matching
// signed install still succeeds and unsigned installs still proceed.

#[test]
fn a_signature_cannot_bless_a_different_applet_id() {
    // review 083 #1: a valid T012 signed package whose signed `appId` is
    // `app.notes` must NOT be installable under a DIFFERENT local applet id. The
    // code + manifest match the signed package exactly, so only the appId differs
    // — and that alone must reject, otherwise a valid signature for one app
    // identity blesses an unrelated local id (provenance/upgrade identity unbound).
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    // The signed appId is `app.notes`; install under a different id.
    let resp = install_demo_signed(&mut core, "some.other.app", &fixture, signature_block_from_fixture(&fixture));

    assert!(!resp.ok, "a signed package installed under a different applet id must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("appId")
            && err.to_string().contains("installed applet id"),
        "the appId mismatch is surfaced: {err}"
    );

    // Nothing was installed under the borrowed id.
    let run = core.handle(cmd("runtime.run", Some("some.other.app"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "the rejected install stored nothing");
}

#[test]
fn a_signature_cannot_widen_any_enforced_limit() {
    // review 083 #2: the runtime enforces fuel, max_host_calls, storage_bytes,
    // log_bytes (and wall_ms/memory_bytes) from the stored top-level manifest. The
    // signed `valid_signature` package's resourceBudget declares ONLY
    // wall_ms/memory_bytes, so the other four are bound to the runtime default. A
    // signed install that widens ANY of the six must be rejected — exercise each.
    let fixture = load_signing_fixture("valid_signature.json");
    let widen = [
        ("fuel", serde_json::json!(20_000_000u64)),
        ("max_host_calls", serde_json::json!(1_000_000u64)),
        ("storage_bytes", serde_json::json!(1_000_000_000u64)),
        ("log_bytes", serde_json::json!(10_000_000u64)),
        ("memory_bytes", serde_json::json!(134_217_728u64)),
        ("wall_ms", serde_json::json!(60_000u64)),
    ];
    for (field, wider) in widen {
        let mut core = WorkspaceCore::in_memory("ws1").unwrap();
        let mut manifest = signed_fixture_manifest();
        manifest["limits"][field] = wider.clone();
        let resp = core.handle(cmd(
            "applet.install",
            Some(SIGNED_APP_ID),
            serde_json::json!({
                "manifest": manifest,
                "sources": sources_from_fixture(&fixture),
                "signature": signature_block_from_fixture(&fixture),
            }),
        ));
        assert!(!resp.ok, "widening limits.{field} beyond the signed boundary must be rejected");
        let err = resp.error.expect("must carry an error");
        assert_eq!(err.code(), "ValidationError");
        assert!(
            err.to_string().contains(&format!("limits.{field}")),
            "the {field} limit mismatch is surfaced: {err}"
        );
    }
}

#[test]
fn a_signature_cannot_loosen_a_net_cap_or_add_a_secret_header() {
    // review 083 #3: the whole NetRule is bound, not just (method, url). The
    // signed `valid_signature` package allows NO network, so to exercise the full
    // net comparison we install under a signed package that DOES declare a net
    // allow rule and then mutate one constraint per case. Build that signed-shaped
    // expectation by mutating both sides consistently is complex; instead use the
    // simpler, equally strict direction the bind guarantees: the signed package
    // allows zero net, so an install adding ANY net rule (with a secret header)
    // must be rejected — the net sets differ.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let mut manifest = signed_fixture_manifest();
    // The signed package's networkPolicy.allow is empty; add a rule with a loose
    // cap AND a secret header the publisher never blessed.
    manifest["capabilities"]["net"] = serde_json::json!([
        {
            "method": "GET",
            "url": "https://api.example.com/private/*",
            "max_response_bytes": 9_999_999u64,
            "allow_secret_headers": ["Authorization"]
        }
    ]);
    let resp = core.handle(cmd(
        "applet.install",
        Some(SIGNED_APP_ID),
        serde_json::json!({
            "manifest": manifest,
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));

    assert!(!resp.ok, "adding a net rule with a secret header the publisher never signed must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("network egress grant"),
        "the net-rule mismatch is surfaced: {err}"
    );
    assert!(
        err.to_string().contains("secret_hdrs") && err.to_string().contains("Authorization"),
        "the full net rule (incl. secret headers) is compared, not just routing: {err}"
    );
}

/// The install manifest that EXACTLY matches the `bind_net_rule`
/// signed package: empty storage/db, `ui`, and ONE net rule
/// (`GET https://api.example.com/v1/*`, `max_response_bytes: 1000`). The four
/// limits the signed budget omits keep the M0a defaults.
fn net_rule_fixture_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true,
            "net": [
                { "method": "GET", "url": "https://api.example.com/v1/*", "max_response_bytes": 1000 }
            ]
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    })
}

#[test]
fn a_signed_net_rule_install_that_matches_the_signed_cap_succeeds() {
    // review 086 #2 positive baseline: the `bind_net_rule` signed
    // package allows EXACTLY one net rule. An install carrying that same rule with
    // the SAME cap must install as Signed — this proves the fixture is genuinely
    // installable, so the rejection in the sibling test is the cap mismatch, not a
    // broken fixture.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("bind_net_rule.json");
    let resp = core.handle(cmd(
        "applet.install",
        Some("app.netnotes"),
        serde_json::json!({
            "manifest": net_rule_fixture_manifest(),
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));
    assert!(resp.ok, "a net-rule install matching the signed cap must install: {:?}", resp.error);
    let trust = resp.payload["trust"]["status"].clone();
    assert_eq!(trust, serde_json::json!("signed"), "it must record Signed trust");
}

#[test]
fn a_signature_with_a_net_rule_cannot_be_widened_to_a_larger_cap() {
    // review 086 #2 (pins review 083 #3): the prior net regression test only
    // exercised the EMPTY-allow direction, so a binder that compared just
    // (method, url) would still pass it. Here the signed package allows ONE rule
    // with `max_response_bytes: 1000`; install the SAME method/url but WIDEN the
    // cap (and, separately, add an unblessed secret header). Both must be rejected
    // because the FULL normalized NetRule — not just routing — is bound.
    let fixture = load_signing_fixture("bind_net_rule.json");

    // (a) a wider response-byte cap than the signed rule blessed.
    {
        let mut core = WorkspaceCore::in_memory("ws1").unwrap();
        let mut manifest = net_rule_fixture_manifest();
        manifest["capabilities"]["net"][0]["max_response_bytes"] = serde_json::json!(1_000_000u64);
        let resp = core.handle(cmd(
            "applet.install",
            Some("app.netnotes"),
            serde_json::json!({
                "manifest": manifest,
                "sources": sources_from_fixture(&fixture),
                "signature": signature_block_from_fixture(&fixture),
            }),
        ));
        assert!(!resp.ok, "widening max_response_bytes past the signed cap must be rejected");
        let err = resp.error.expect("must carry an error");
        assert_eq!(err.code(), "ValidationError");
        assert!(
            err.to_string().contains("network egress grant"),
            "the net-rule mismatch is surfaced: {err}"
        );
    }

    // (b) the same routing, but an extra secret header the publisher never signed.
    {
        let mut core = WorkspaceCore::in_memory("ws1").unwrap();
        let mut manifest = net_rule_fixture_manifest();
        manifest["capabilities"]["net"][0]["allow_secret_headers"] =
            serde_json::json!(["Authorization"]);
        let resp = core.handle(cmd(
            "applet.install",
            Some("app.netnotes"),
            serde_json::json!({
                "manifest": manifest,
                "sources": sources_from_fixture(&fixture),
                "signature": signature_block_from_fixture(&fixture),
            }),
        ));
        assert!(!resp.ok, "adding an unblessed secret header to a signed net rule must be rejected");
        let err = resp.error.expect("must carry an error");
        assert_eq!(err.code(), "ValidationError");
        assert!(
            err.to_string().contains("secret_hdrs") && err.to_string().contains("Authorization"),
            "the full net rule (incl. secret headers) is compared, not just routing: {err}"
        );
    }
}

#[test]
fn a_signed_package_with_an_unsupported_budget_field_is_refused() {
    // review 086 #1: the signed manifest is hashed/signed WHOLE, so a package can
    // carry a future, tighter constraint this core does not understand. The
    // `bind_unknown_budget_field` fixture is validly signed but its
    // `resourceBudget` declares `network_bytes`, a limit this core cannot enforce.
    // Installing it as Signed would silently DROP that constraint, so the bind must
    // fail closed (prd-merged/08 §08:24) rather than report Signed. Crypto and
    // integrity pass (the signature is valid over the whole manifest); the refusal
    // is a compat/permission decision, not a tamper detection.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("bind_unknown_budget_field.json");
    // A manifest that matches the signed (today-only) shape; the gap is purely the
    // unknown signed budget key, which has no install-side representation.
    let manifest = serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    let resp = core.handle(cmd(
        "applet.install",
        Some("app.future"),
        serde_json::json!({
            "manifest": manifest,
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));
    assert!(!resp.ok, "a signed package declaring an unsupported budget limit must be refused");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("resourceBudget") && err.to_string().contains("network_bytes"),
        "the refusal names the unsupported resourceBudget field: {err}"
    );
}

#[test]
fn a_signed_multi_file_install_is_rejected_until_entrypoint_is_representable() {
    // review 083 #4: the signed manifest carries no entrypoint, so a signed
    // MULTI-FILE package cannot pin which file runs — a caller could otherwise
    // pick any signed file as the entrypoint. Until the signed manifest can
    // represent the entrypoint, signed multi-file installs are rejected. (The
    // `valid_multi_file_package` T012 fixture is a valid two-file signed package.)
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_multi_file_package.json");
    // A manifest matching the signed `app.multi` package's (empty) capabilities,
    // picking `src/ui.ts` as the runnable entrypoint — a non-`main` signed file.
    let manifest = serde_json::json!({
        "entrypoint": "src/ui.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    let resp = core.handle(cmd(
        "applet.install",
        Some("app.multi"),
        serde_json::json!({
            "manifest": manifest,
            "sources": sources_from_fixture(&fixture),
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));

    assert!(!resp.ok, "a signed multi-file install must be rejected until entrypoint is representable");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("multi-file"),
        "the multi-file rejection is surfaced: {err}"
    );

    // Nothing was installed.
    let run = core.handle(cmd("runtime.run", Some("app.multi"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "the rejected multi-file install stored nothing");
}

#[test]
fn an_unsigned_install_still_proceeds() {
    // review 083: the bind only tightens the SIGNED path. An install with NO
    // signature must still proceed (signing is not mandatory in M0a) — the demo
    // path is untouched and reports `unsigned`.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core, DEMO_TS, demo_manifest());
    // install_demo asserts ok internally; confirm it runs (unsigned, untouched path).
    let run = core.handle(cmd("runtime.run", Some("app_demo"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "an unsigned applet still installs and runs: {:?}", run.error);
}

#[test]
fn a_valid_signature_cannot_bless_different_top_level_code() {
    // review 080 #1: a valid T012 signed package attached to an install whose
    // top-level `sources` are NOT the signed files must be REJECTED — otherwise a
    // caller could borrow any valid signature to bless arbitrary code. The
    // signature verifies (crypto + integrity are fine), but the install ships
    // `DEMO_TS` instead of the signed `src/main.ts`, so the bind-to-payload check
    // rejects at the package_hash layer and nothing is stored.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_borrowed_sig"),
        serde_json::json!({
            "manifest": demo_manifest(),
            // The signed package's file is `return { ok: true }`; ship different code.
            "sources": { "src/main.ts": DEMO_TS },
            "signature": signature_block_from_fixture(&fixture),
        }),
    ));

    assert!(!resp.ok, "a borrowed signature over different code must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("package signature invalid")
            && err.to_string().contains("package_hash"),
        "the bind-to-payload mismatch is surfaced at package_hash: {err}"
    );

    // Nothing was installed.
    let run = core.handle(cmd("runtime.run", Some("app_borrowed_sig"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "the rejected install stored nothing");
}

#[test]
fn install_signed_package_with_trusted_publisher_enforces_policy_layer() {
    // When the install carries a publisher-trust block, the marketplace-policy
    // layer is ENFORCED (SC-15 policy-vs-crypto split). A trusted, unexpired
    // publisher matching the package installs OK and reports the policy was
    // enforced. (The `valid_signature` fixture's publisher is `test-publisher`.)
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("valid_signature.json");
    let mut signature = signature_block_from_fixture(&fixture);
    signature["publisher_trust"] = serde_json::json!({
        "publisher": "test-publisher",
        "trusted": true,
        "valid_until": serde_json::Value::Null,
    });
    let resp = install_demo_signed(&mut core, SIGNED_APP_ID, &fixture, signature);

    assert!(resp.ok, "trusted publisher installs: {:?}", resp.error);
    assert_eq!(resp.payload["trust"]["status"], serde_json::json!("signed"));
    assert_eq!(
        resp.payload["trust"]["publisher_trust_enforced"],
        serde_json::json!(true),
        "the policy layer was enforced because a publisher_trust block was supplied"
    );
}

#[test]
fn install_tampered_signed_package_is_rejected_and_not_installed() {
    // A signed package whose file content was changed after signing: the
    // signature still verifies over the (unchanged) recorded-hash preimage, but
    // the live content no longer matches the signed contentHash, so verify_package
    // rejects at the package_hash (integrity) layer. The install must be REJECTED
    // with a ValidationError and NOTHING stored.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("invalid_file_content_hash_mismatch.json");
    let sig = signature_block_from_fixture(&fixture);
    let resp = install_demo_signed(&mut core, "app_tampered", &fixture, sig);

    assert!(!resp.ok, "a tampered signed package must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError", "tamper → ValidationError: {err}");
    assert!(
        err.to_string().contains("package signature invalid"),
        "the rejection names the signature failure: {err}"
    );
    assert!(
        err.to_string().contains("package_hash"),
        "the integrity (package_hash) failure layer is surfaced: {err}"
    );

    // Nothing was installed: a subsequent run reports the applet missing.
    let run = core.handle(cmd("runtime.run", Some("app_tampered"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "the rejected applet was never stored");
    assert_eq!(run.error.unwrap().code(), "ValidationError");
}

#[test]
fn install_with_a_garbage_signature_is_rejected_at_the_crypto_layer() {
    // A garbage Ed25519 signature over an otherwise-intact package: verify_package
    // rejects at the crypto layer. The install is rejected with the crypto layer
    // surfaced — distinct from the integrity (package_hash) rejection above.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("invalid_garbage_signature.json");
    let sig = signature_block_from_fixture(&fixture);
    let resp = install_demo_signed(&mut core, "app_garbage", &fixture, sig);

    assert!(!resp.ok, "a garbage signature must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("package signature invalid")
            && err.to_string().contains("crypto"),
        "the crypto failure layer is surfaced: {err}"
    );
}

#[test]
fn install_with_an_untrusted_publisher_is_rejected_at_the_policy_layer() {
    // The package's crypto + integrity are fine, but the supplied publisher-trust
    // block marks the publisher `unknown` (not in the trusted set), so the
    // marketplace-policy layer rejects the install. This is the policy-vs-crypto
    // split: a valid signature is not enough when the installer does not trust the
    // publisher (the `invalid_unknown_publisher` T012 vector).
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let fixture = load_signing_fixture("invalid_unknown_publisher.json");
    let sig = signature_block_from_fixture(&fixture);
    let resp = install_demo_signed(&mut core, "app_untrusted", &fixture, sig);

    assert!(!resp.ok, "an untrusted publisher must be rejected");
    let err = resp.error.expect("must carry an error");
    assert_eq!(err.code(), "ValidationError");
    assert!(
        err.to_string().contains("package signature invalid")
            && err.to_string().contains("policy"),
        "the policy failure layer is surfaced: {err}"
    );
}

#[test]
fn install_without_a_signature_proceeds_unsigned() {
    // No `signature` field: the install proceeds (M0a — signing is not yet
    // mandatory) and the response indicates `unsigned`. This is the existing
    // demo/spine path, unchanged.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_unsigned"),
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": DEMO_TS },
        }),
    ));

    assert!(resp.ok, "an unsigned install still succeeds: {:?}", resp.error);
    assert_eq!(
        resp.payload["trust"]["status"],
        serde_json::json!("unsigned"),
        "the response indicates the applet was installed unsigned"
    );

    // The unsigned applet runs exactly as before (no regression).
    let run = core.handle(cmd("runtime.run", Some("app_unsigned"), serde_json::json!({ "input": {} })));
    assert!(run.ok, "the unsigned applet runs: {:?}", run.error);
}

#[test]
fn install_with_a_malformed_signature_block_is_a_validation_error() {
    // A `signature` field that is present but missing required sub-fields is a
    // ValidationError (no panic/unwrap on the real path), and nothing is stored.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_malformed"),
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": DEMO_TS },
            "signature": { "signature": "ed25519:AAAA" }, // missing `package` + `public_key`
        }),
    ));
    assert!(!resp.ok, "a malformed signature block must be rejected");
    assert_eq!(resp.error.unwrap().code(), "ValidationError");

    let run = core.handle(cmd("runtime.run", Some("app_malformed"), serde_json::json!({ "input": {} })));
    assert!(!run.ok, "nothing was installed");
}
