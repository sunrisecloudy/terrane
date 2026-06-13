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
    core.grant_db_read("dev", ["tasks"]);

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

/// Review 048 finding 1: the `db.read` scope must NOT be forgeable from the
/// request body. An actor trusted to read only `tasks` cannot reach an ungranted
/// collection by (a) omitting `grants`, or (b) self-expanding `grants` to `["*"]`
/// / the target collection in the payload. The trusted table is the only source
/// of truth; a payload that tries to widen access is rejected, never honored.
#[test]
fn query_execute_db_read_scope_is_not_forgeable_from_payload() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    // Owner ("dev") is trusted to read ONLY `tasks`.
    core.grant_db_read("dev", ["tasks"]);

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
