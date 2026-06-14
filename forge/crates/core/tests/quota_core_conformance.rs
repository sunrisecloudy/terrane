//! DL-22 quota LIVE-WIRING conformance over `forge/fixtures/quotas-core/`
//! (manifest `count = 6`).
//!
//! This is the forge-CORE half of the DL-22 contract: every vector drives the REAL
//! command/host boundary — [`WorkspaceCore::handle`] (`runtime.run` / `quota.status` /
//! `quota.set`) and the [`WorkspaceCore::put_attachment`] workspace seam — NOT a
//! storage unit. (`forge-storage/tests/quota_fixtures.rs` pins the pure
//! decision/accounting/dedup substrate; this pins that the production code paths
//! enforce + report through it, the live-wiring lesson.)
//!
//! By `kind`:
//!
//! - `db_write`: install a demo applet whose first effect is `ctx.db.insert`, seed N
//!   runs, apply the trusted `quota.set` (a policy expressed RELATIVE to the seeded
//!   usage so it is robust to the exact chunk bytes), then run once more. The vector
//!   asserts the run is accepted (optionally with an approaching warning surfaced as
//!   both the response `quota_warnings` field AND a `quota.approaching` event) or
//!   REJECTED with a typed `ResourceLimitExceeded` + the compaction/cleanup/export
//!   suggestion — and, on rejection, that the prior records + the records usage are
//!   byte-for-byte intact and the rejected record never landed (reject-not-delete).
//! - `attachment`: `put_attachment` the listed bytes through the workspace seam and
//!   assert dedup (one stored blob per content hash, refcounted) + that `quota.status`
//!   attachments bytes are unchanged across a dedup hit.
//! - `status`: seed runs, then `quota.status`; assert the usage shape, the effective
//!   DL-22 default policy, the (empty) approaching list, and two byte-equal reads.
//! - `set`: a non-owner `quota.set` is `PermissionDenied`; an Owner set persists and is
//!   read back by `quota.status`; an invalid set is a `ValidationError`.
//!
//! The relative-policy directive `workspace_limit_pct_of_current: P` sets
//! `workspace_limit` so the CURRENT seeded usage is exactly `P%` of it
//! (`limit = ceil(current * 100 / P)`): `P = 100` ⇒ zero headroom (the next write is
//! over quota); `P = 80` ⇒ current sits at the 80% approaching threshold and one more
//! small write stays under the limit but in the approaching band.
//!
//! A `ran == manifest.count` guard makes a dropped / misnamed / unhandled vector FAIL
//! the suite rather than silently pass, and at least one vector
//! (`over_quota_db_write_rejected_data_intact`) drives a REAL over-quota `ctx.db`
//! write that is rejected with data intact (the live proof).

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, ActorId, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/quotas-core")
}

fn load(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
}

/// The demo applet: its FIRST effect is `ctx.db.insert("tasks", …)` so an over-quota
/// or approaching write is decided on that real records write.
const DEMO_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const title: string = input && input.title ? input.title : "untitled";
        const id = await ctx.db.insert("tasks", { title: title, done: false });
        return { ok: true, value: { id: id } };
    }
"#;

fn demo_manifest() -> Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "db": { "read": ["tasks"], "write": ["tasks"] }
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

fn actor(role: Role) -> ActorContext {
    ActorContext { actor: ActorId::new(format!("{role:?}").to_lowercase()), role }
}

fn cmd_as(actor: ActorContext, name: &str, applet_id: Option<&str>, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor,
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: applet_id.map(AppletId::new),
        name: name.into(),
        payload,
    }
}

fn cmd(name: &str, applet_id: Option<&str>, payload: Value) -> CoreCommand {
    cmd_as(owner(), name, applet_id, payload)
}

fn install_demo(core: &mut WorkspaceCore) {
    let resp = core.handle(cmd(
        "applet.install",
        Some("app_demo"),
        serde_json::json!({ "manifest": demo_manifest(), "sources": { "src/main.ts": DEMO_TS } }),
    ));
    assert!(resp.ok, "install must succeed: {:?}", resp.error);
}

/// Drive one `runtime.run` with `{ title }` input. Returns the (ok, response payload).
fn run_with_title(core: &mut WorkspaceCore, title: &str) -> forge_domain::CoreResponse {
    core.handle(cmd(
        "runtime.run",
        Some("app_demo"),
        serde_json::json!({ "input": { "title": title } }),
    ))
}

/// The number of rows `query.execute` returns for `tasks` (through the real command
/// boundary).
fn task_count(core: &mut WorkspaceCore) -> usize {
    let resp = core.handle(cmd("query.execute", None, serde_json::json!({ "collection": "tasks" })));
    assert!(resp.ok, "query.execute must succeed: {:?}", resp.error);
    resp.payload["rows"].as_array().expect("rows").len()
}

/// Apply a fixture `set_policy` directive (or `null`). A `workspace_limit_pct_of_current`
/// directive is resolved RELATIVE to the current seeded usage; an explicit field is set
/// verbatim. Returns once the trusted policy is persisted (or immediately for `null`).
fn apply_set_policy(core: &mut WorkspaceCore, set: &Value) {
    if set.is_null() {
        return;
    }
    let current = core.quota_usage().unwrap().workspace_total_bytes;
    let mut policy = serde_json::Map::new();
    if let Some(pct) = set.get("workspace_limit_pct_of_current").and_then(Value::as_u64) {
        // limit = ceil(current * 100 / pct): current sits at exactly `pct`% of the limit.
        let limit = current.saturating_mul(100).div_ceil(pct);
        policy.insert("workspace_limit".into(), serde_json::json!(limit));
    }
    if let Some(x) = set.get("approaching_threshold").and_then(Value::as_f64) {
        policy.insert("approaching_threshold".into(), serde_json::json!(x));
    }
    let resp = core.handle(cmd(
        "quota.set",
        None,
        serde_json::json!({ "policy": Value::Object(policy) }),
    ));
    assert!(resp.ok, "quota.set must succeed: {:?}", resp.error);
}

fn run_db_write(case: &str, fx: &Value) {
    let a = &fx["assert"];
    let collection = a["collection"].as_str().unwrap();
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);

    // Seed the workspace so usage is non-trivial, then snapshot the records substrate.
    for seed in a["seed_runs"].as_array().unwrap() {
        let resp = run_with_title(&mut core, seed["title"].as_str().unwrap());
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "{case}: seed run must succeed");
    }
    apply_set_policy(&mut core, &a["set_policy"]);

    let before_usage = core.quota_usage().unwrap();
    let before_rows = core.handle(cmd("query.execute", None, serde_json::json!({ "collection": collection })));
    let before_rows = before_rows.payload["rows"].as_array().unwrap().clone();
    let before_count = before_rows.len();

    // Drain prior events so we observe ONLY this run's events.
    core.events_mut().drain();
    let resp = run_with_title(&mut core, a["write_run"]["title"].as_str().unwrap());
    assert!(resp.ok, "{case}: the runtime.run command itself must return Ok (the RUN may fail): {:?}", resp.error);
    let run_ok = resp.payload["ok"] == Value::Bool(true);
    let expect = &a["expect"];

    assert_eq!(
        run_ok,
        expect["run_ok"].as_bool().unwrap(),
        "{case}: run_ok mismatch (payload={})",
        resp.payload
    );

    let approaching_events = core.events().events_of_kind("quota.approaching").count();

    if expect["run_ok"].as_bool() == Some(true) {
        // ACCEPTED path: optionally an approaching warning surfaced on BOTH the
        // response field and the event stream.
        let warnings = resp.payload["quota_warnings"].as_array().expect("quota_warnings field");
        if expect["warns"].as_bool() == Some(true) {
            assert!(!warnings.is_empty(), "{case}: expected an approaching warning, got none");
            let w = &warnings[0];
            if let Some(scope) = expect["warning_scope"].as_str() {
                assert_eq!(w["scope"].as_str(), Some(scope), "{case}: warning scope");
            }
            if let Some(contains) = expect["warning_suggestion_contains"].as_str() {
                assert!(
                    w["suggestion"].as_str().unwrap().contains(contains),
                    "{case}: suggestion must contain {contains:?}, got {}",
                    w["suggestion"]
                );
            }
            // The projected post-write total must be at/above the threshold of its limit.
            let projected = w["projected"].as_u64().unwrap();
            let limit = w["limit"].as_u64().unwrap();
            assert!(projected <= limit, "{case}: an approaching (not over) write stays within the limit");
            if expect["approaching_event_emitted"].as_bool() == Some(true) {
                assert_eq!(approaching_events, warnings.len(), "{case}: one event per warning");
            }
        } else {
            assert!(warnings.is_empty(), "{case}: expected NO warning, got {warnings:?}");
            if expect["approaching_event_emitted"].as_bool() == Some(false) {
                assert_eq!(approaching_events, 0, "{case}: no quota.approaching event under limit");
            }
        }
        return;
    }

    // REJECTED path: the run FAILED with a typed ResourceLimitExceeded, and the
    // records substrate is byte-for-byte intact (reject-not-delete).
    let error = &resp.payload["result"]["error"];
    if let Some(code) = expect["error_code"].as_str() {
        assert_eq!(error["kind"].as_str(), Some(code), "{case}: error kind (payload={})", resp.payload);
    }
    if let Some(contains) = expect["error_contains"].as_str() {
        assert!(
            error["detail"].as_str().unwrap_or_default().contains(contains),
            "{case}: error detail must contain {contains:?}, got {error}"
        );
    }
    if expect["prior_records_intact"].as_bool() == Some(true) {
        let after_rows = core.handle(cmd(
            "query.execute",
            None,
            serde_json::json!({ "collection": collection }),
        ));
        let after_rows = after_rows.payload["rows"].as_array().unwrap();
        assert_eq!(after_rows, &before_rows, "{case}: prior records must be byte-for-byte intact");
    }
    if expect["record_count_unchanged"].as_bool() == Some(true) {
        assert_eq!(task_count(&mut core), before_count, "{case}: the rejected record must not land");
    }
    if expect["records_usage_unchanged_after_reject"].as_bool() == Some(true) {
        // The records substrate (per-applet collection bytes + retained_chunks) must be
        // unchanged — the over-quota records write rolled back. (Whole-workspace usage
        // legitimately grows because the FAILED run itself is recorded in run_logs.)
        let after_usage = core.quota_usage().unwrap();
        let applet = forge_storage::applet_of_collection(collection);
        assert_eq!(
            after_usage.applet_bytes(applet),
            before_usage.applet_bytes(applet),
            "{case}: per-applet records bytes unchanged after a rejected write"
        );
        assert_eq!(
            after_usage.category_bytes(forge_storage::QuotaCategory::RetainedChunks),
            before_usage.category_bytes(forge_storage::QuotaCategory::RetainedChunks),
            "{case}: retained_chunks bytes unchanged after a rejected write"
        );
    }
}

fn run_attachment(case: &str, fx: &Value) {
    let a = &fx["assert"];
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    let mut hashes: Vec<String> = Vec::new();
    let mut attach_bytes: Vec<u64> = Vec::new();
    for put in a["puts"].as_array().unwrap() {
        let bytes = put["bytes"].as_str().unwrap().as_bytes();
        let res = core.put_attachment(bytes).unwrap();
        assert_eq!(
            res.stored_new,
            put["expect_stored_new"].as_bool().unwrap(),
            "{case}: stored_new for {:?}",
            put["bytes"]
        );
        assert_eq!(res.refcount, put["expect_refcount"].as_u64().unwrap(), "{case}: refcount");
        hashes.push(res.content_hash);
        // Read attachments bytes back through the quota.status COMMAND.
        attach_bytes.push(status_category_bytes(&mut core, "attachments"));
    }

    let expect = &a["expect"];
    if expect["first_and_second_same_hash"].as_bool() == Some(true) {
        assert_eq!(hashes[0], hashes[1], "{case}: identical bytes share a content hash");
    }
    if expect["third_distinct_hash"].as_bool() == Some(true) {
        assert_ne!(hashes[0], hashes[2], "{case}: distinct bytes get a distinct hash");
    }
    if expect["attachments_status_bytes_unchanged_between_puts_1_and_2"].as_bool() == Some(true) {
        assert_eq!(attach_bytes[0], attach_bytes[1], "{case}: a dedup hit adds no accounted bytes");
    }
    if expect["attachments_status_bytes_increases_on_third"].as_bool() == Some(true) {
        assert!(attach_bytes[2] > attach_bytes[1], "{case}: a distinct blob adds accounted bytes");
    }
}

/// The `attachments` (or any) category bytes as reported by the `quota.status` command.
fn status_category_bytes(core: &mut WorkspaceCore, category: &str) -> u64 {
    let resp = core.handle(cmd("quota.status", None, serde_json::json!({})));
    assert!(resp.ok, "quota.status must succeed: {:?}", resp.error);
    resp.payload["usage"]["per_category"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["category"].as_str() == Some(category))
        .and_then(|c| c["bytes"].as_u64())
        .unwrap_or_else(|| panic!("quota.status missing category {category}"))
}

fn run_status(case: &str, fx: &Value) {
    let a = &fx["assert"];
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_demo(&mut core);
    for seed in a["seed_runs"].as_array().unwrap() {
        let resp = run_with_title(&mut core, seed["title"].as_str().unwrap());
        assert!(resp.ok && resp.payload["ok"] == Value::Bool(true), "{case}: seed run must succeed");
    }

    let resp = core.handle(cmd("quota.status", None, serde_json::json!({})));
    assert!(resp.ok, "{case}: quota.status must succeed: {:?}", resp.error);
    let usage = &resp.payload["usage"];
    let policy = &resp.payload["policy"];
    let approaching = resp.payload["approaching"].as_array().unwrap();
    let expect = &a["expect"];

    // Per-applet report (sorted, the deterministic shape).
    let applets: Vec<&str> = usage["per_applet"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["applet"].as_str().unwrap())
        .collect();
    let want_applets: Vec<&str> = expect["applets_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(applets, want_applets, "{case}: per-applet report");

    // Per-category report shape.
    let categories: Vec<&str> = usage["per_category"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["category"].as_str().unwrap())
        .collect();
    let want_categories: Vec<&str> = expect["categories_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(categories, want_categories, "{case}: per-category report shape");

    if expect["retained_chunks_bytes_positive"].as_bool() == Some(true) {
        let bytes = usage["per_category"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["category"] == "retained_chunks")
            .and_then(|c| c["bytes"].as_u64())
            .unwrap();
        assert!(bytes > 0, "{case}: retained_chunks bytes positive");
    }
    if expect["workspace_total_is_sum_of_applets_plus_categories"].as_bool() == Some(true) {
        let applets_total: u64 = usage["per_applet"].as_array().unwrap().iter().map(|a| a["collections_bytes"].as_u64().unwrap()).sum();
        let categories_total: u64 = usage["per_category"].as_array().unwrap().iter().map(|c| c["bytes"].as_u64().unwrap()).sum();
        assert_eq!(
            usage["workspace_total_bytes"].as_u64().unwrap(),
            applets_total + categories_total,
            "{case}: workspace total is the sum of every accounted slice"
        );
    }
    if expect["policy_is_dl22_defaults"].as_bool() == Some(true) {
        assert_eq!(policy["workspace_limit"].as_u64(), Some(forge_storage::GIB), "{case}: default workspace_limit");
        assert_eq!(policy["per_applet_limit"].as_u64(), Some(100 * forge_storage::MIB), "{case}: default per_applet_limit");
        assert_eq!(policy["approaching_threshold"].as_f64(), Some(0.8), "{case}: default threshold");
    }
    if expect["approaching_is_empty"].as_bool() == Some(true) {
        assert!(approaching.is_empty(), "{case}: nothing approaching under the roomy defaults");
    }
    if expect["two_reads_byte_equal"].as_bool() == Some(true) {
        let again = core.handle(cmd("quota.status", None, serde_json::json!({})));
        assert_eq!(again.payload, resp.payload, "{case}: two quota.status reads are byte-equal");
    }
}

fn run_set(case: &str, fx: &Value) {
    let a = &fx["assert"];
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // A non-owner role is rejected at the command-RBAC gate BEFORE any state change.
    let role = match a["non_owner_role"].as_str().unwrap() {
        "Editor" => Role::Editor,
        "Viewer" => Role::Viewer,
        "Maintainer" => Role::Maintainer,
        "Runner" => Role::Runner,
        "Auditor" => Role::Auditor,
        other => panic!("{case}: unhandled non_owner_role {other}"),
    };
    let denied = core.handle(cmd_as(
        actor(role),
        "quota.set",
        None,
        serde_json::json!({ "policy": { "workspace_limit": 1 } }),
    ));
    assert!(!denied.ok, "{case}: a non-owner quota.set must be denied");
    assert_eq!(
        denied.error.as_ref().map(|e| e.code()),
        Some(a["expect_non_owner_error_code"].as_str().unwrap()),
        "{case}: non-owner denial code"
    );

    // An Owner set persists the trusted override; quota.status reads it back.
    let set = core.handle(cmd("quota.set", None, serde_json::json!({ "policy": a["owner_set_policy"] })));
    assert!(set.ok, "{case}: owner quota.set must succeed: {:?}", set.error);
    let status = core.handle(cmd("quota.status", None, serde_json::json!({})));
    let policy = &status.payload["policy"];
    let want = &a["expect_after_owner_set"];
    assert_eq!(policy["workspace_limit"].as_u64(), want["workspace_limit"].as_u64(), "{case}: workspace_limit read back");
    assert_eq!(policy["per_applet_limit"].as_u64(), want["per_applet_limit"].as_u64(), "{case}: per_applet_limit read back");
    assert_eq!(policy["approaching_threshold"].as_f64(), want["approaching_threshold"].as_f64(), "{case}: threshold read back");

    // An invalid override is rejected (config validation), leaving the prior policy.
    let invalid = core.handle(cmd("quota.set", None, serde_json::json!({ "policy": a["invalid_set_policy"] })));
    assert!(!invalid.ok, "{case}: an invalid quota.set must be rejected");
    assert_eq!(
        invalid.error.as_ref().map(|e| e.code()),
        Some(a["expect_invalid_error_code"].as_str().unwrap()),
        "{case}: invalid override code"
    );
    let after = core.handle(cmd("quota.status", None, serde_json::json!({})));
    assert_eq!(
        after.payload["policy"]["workspace_limit"].as_u64(),
        want["workspace_limit"].as_u64(),
        "{case}: a rejected invalid set leaves the prior trusted policy unchanged"
    );
}

#[test]
fn dl22_quota_core_conformance() {
    let manifest = load("manifest.json");
    let cases = manifest["cases"].as_array().expect("manifest cases");
    let declared = manifest["count"].as_u64().expect("manifest count") as usize;
    assert_eq!(cases.len(), declared, "manifest count must match listed cases");

    let mut ran = 0usize;
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let kind = case["kind"].as_str().unwrap();
        let fx = load(case["file"].as_str().unwrap());
        assert_eq!(
            fx["assert"]["kind"].as_str().unwrap(),
            kind,
            "{name}: manifest kind must match the fixture assert kind"
        );
        match kind {
            "db_write" => run_db_write(name, &fx),
            "attachment" => run_attachment(name, &fx),
            "status" => run_status(name, &fx),
            "set" => run_set(name, &fx),
            other => panic!("{name}: unknown assert kind {other}"),
        }
        ran += 1;
    }
    assert_eq!(ran, declared, "every quota-core fixture must run");
}
