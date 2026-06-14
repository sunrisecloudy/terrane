//! Data-driven DL-22 quota harness over `forge/fixtures/quotas/`.
//!
//! Each fixture asserts one of five behaviors by its manifest `kind`:
//!
//! - `decision`: a synthetic `(usage, policy, category, applet, write_bytes)` fed to
//!   the pure `decide_quota` must yield the listed `Ok` / `ApproachingLimit` /
//!   `OverQuota` decision (and, for a rejection, the typed error code + suggestion).
//! - `enforce`: on the REAL DL-4 records write path (`apply_mutation_crdt`), with the
//!   listed trusted policy override, the listed mutation is accepted or rejected; a
//!   rejected write leaves the prior record + accounted usage byte-for-byte intact
//!   and never lands the rejected record (reject-not-delete).
//! - `dedup`: `Store::put_attachment` stores identical bytes ONCE (refcounted) and
//!   accounts the attachments category once; distinct bytes add a second blob.
//! - `report`: `Store::quota_usage` reports the expected per-applet / per-category /
//!   workspace shape, summed purely from persisted state; two reads are byte-equal.
//! - `policy`: the const `DEFAULT` applies with no override; a persisted override is
//!   read back; an invalid override is rejected (config is trusted state).
//!
//! A `ran == manifest.count` guard makes a silently-skipped case fail, and every
//! assertion reads back the real stored substrate (no faking).

use forge_storage::{
    decide_quota, AppletUsage, CategoryUsage, IndexManager, Mutation, QuotaCategory, QuotaDecision,
    QuotaPolicy, QuotaScope, QuotaUsage, Store,
};
use serde_json::Value;

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/quotas")
}

fn load(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
}

fn obj(v: &Value) -> serde_json::Map<String, Value> {
    v.as_object().expect("fields object").clone()
}

fn category_from_str(s: &str) -> QuotaCategory {
    match s {
        "attachments" => QuotaCategory::Attachments,
        "run_logs" => QuotaCategory::RunLogs,
        "retained_chunks" => QuotaCategory::RetainedChunks,
        "snapshots" => QuotaCategory::Snapshots,
        "cache" => QuotaCategory::Cache,
        other => panic!("unknown category {other}"),
    }
}

/// Build a `QuotaUsage` from a fixture `usage` object: the workspace total, the
/// per-applet list, and the per-category map (filling any unmentioned category 0).
fn usage_from_fixture(v: &Value) -> QuotaUsage {
    let per_applet = v["per_applet"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|a| AppletUsage {
                    applet: a["applet"].as_str().unwrap().to_string(),
                    collections_bytes: a["collections_bytes"].as_u64().unwrap(),
                })
                .collect()
        })
        .unwrap_or_default();
    let per_category = QuotaCategory::ALL
        .iter()
        .map(|&category| CategoryUsage {
            category,
            bytes: v["per_category"]
                .get(category.as_str())
                .and_then(Value::as_u64)
                .unwrap_or(0),
        })
        .collect();
    QuotaUsage {
        workspace_total_bytes: v["workspace_total_bytes"].as_u64().unwrap(),
        per_applet,
        per_category,
    }
}

/// Overlay a fixture `policy` object onto `QuotaPolicy::DEFAULT`: only the named
/// fields are overridden (so a fixture states just the limits it cares about).
fn policy_from_fixture(v: &Value) -> QuotaPolicy {
    let mut p = QuotaPolicy::DEFAULT;
    if let Some(x) = v.get("workspace_limit").and_then(Value::as_u64) {
        p.workspace_limit = x;
    }
    if let Some(x) = v.get("per_applet_limit").and_then(Value::as_u64) {
        p.per_applet_limit = x;
    }
    if let Some(x) = v.get("attachments_cap").and_then(Value::as_u64) {
        p.attachments_cap = x;
    }
    if let Some(x) = v.get("run_logs_cap").and_then(Value::as_u64) {
        p.run_logs_cap = x;
    }
    if let Some(x) = v.get("retained_chunks_cap").and_then(Value::as_u64) {
        p.retained_chunks_cap = x;
    }
    if let Some(x) = v.get("snapshots_cap").and_then(Value::as_u64) {
        p.snapshots_cap = x;
    }
    if let Some(x) = v.get("cache_cap").and_then(Value::as_u64) {
        p.cache_cap = x;
    }
    if let Some(x) = v.get("approaching_threshold").and_then(Value::as_f64) {
        p.approaching_threshold = x;
    }
    p
}

fn insert_mutation(collection: &str, m: &Value) -> Mutation {
    Mutation::Insert {
        collection: collection.into(),
        id: Some(m["id"].as_str().expect("id").to_string()),
        fields: obj(&m["fields"]),
        logical_at: m["logical_at"].as_i64(),
    }
}

fn run_decision(case: &str, fx: &Value) {
    let a = &fx["assert"];
    let usage = usage_from_fixture(&a["usage"]);
    let policy = policy_from_fixture(&a["policy"]);
    let category = category_from_str(a["category"].as_str().unwrap());
    let applet = a["applet"].as_str();
    let write_bytes = a["write_bytes"].as_u64().unwrap();
    let decision = decide_quota(&usage, &policy, category, applet, write_bytes);

    let expect = &a["expect"];
    match expect["decision"].as_str().unwrap() {
        "ok" => assert_eq!(decision, QuotaDecision::Ok, "{case}: expected Ok"),
        "approaching_limit" => {
            assert!(decision.is_approaching(), "{case}: expected approaching, got {decision:?}");
            assert_decision_scope_limits(case, &decision, expect);
        }
        "over_quota" => {
            assert!(decision.is_over_quota(), "{case}: expected over_quota, got {decision:?}");
            assert_decision_scope_limits(case, &decision, expect);
            let err = decision.over_quota_error().expect("over_quota carries an error");
            if let Some(code) = expect["error_code"].as_str() {
                assert_eq!(err.code(), code, "{case}: error code");
            }
            if let Some(contains) = expect["error_contains"].as_str() {
                assert!(
                    format!("{err}").contains(contains),
                    "{case}: error must contain {contains:?}, got {err}"
                );
            }
        }
        other => panic!("{case}: unknown expected decision {other}"),
    }
}

/// Assert the decision's scope + projected/limit numbers match the fixture.
fn assert_decision_scope_limits(case: &str, decision: &QuotaDecision, expect: &Value) {
    let (scope, projected, limit) = match decision {
        QuotaDecision::ApproachingLimit { scope, projected, limit }
        | QuotaDecision::OverQuota { scope, projected, limit } => (scope, *projected, *limit),
        QuotaDecision::Ok => panic!("{case}: Ok has no scope"),
    };
    if let Some(p) = expect["projected"].as_u64() {
        assert_eq!(projected, p, "{case}: projected");
    }
    if let Some(l) = expect["limit"].as_u64() {
        assert_eq!(limit, l, "{case}: limit");
    }
    match expect["scope"].as_str().unwrap() {
        "workspace" => assert_eq!(scope, &QuotaScope::Workspace, "{case}: scope"),
        "applet" => {
            let want = expect["applet"].as_str().unwrap();
            assert_eq!(
                scope,
                &QuotaScope::Applet { applet: want.to_string() },
                "{case}: applet scope"
            );
        }
        "category" => {
            let want = category_from_str(expect["category"].as_str().unwrap());
            assert_eq!(
                scope,
                &QuotaScope::Category { category: want },
                "{case}: category scope"
            );
        }
        other => panic!("{case}: unknown scope {other}"),
    }
}

fn run_enforce(case: &str, fx: &Value) {
    let collection = fx["collection"].as_str().expect("collection").to_string();
    let mut store = Store::open_in_memory().unwrap();
    let idx = IndexManager::new();
    for m in fx["seed"].as_array().expect("seed") {
        store
            .apply_mutation_crdt(&insert_mutation(&collection, m), &idx)
            .expect("seed mutation");
    }

    // Capture the pre-write state for the reject-not-delete proof.
    let usage_before = store.quota_usage().unwrap();
    let doc_id = forge_storage::collection_doc_id(&collection);
    let chunks_before: Vec<String> = store
        .get_chunks(&doc_id)
        .unwrap()
        .into_iter()
        .map(|c| c.chunk_id)
        .collect();

    // Apply the trusted policy override BEFORE the write (config is trusted state).
    let a = &fx["assert"];
    let set = &a["set_policy"];
    let mut policy = QuotaPolicy::DEFAULT;
    if set
        .get("workspace_limit_eq_current_total")
        .and_then(Value::as_bool)
        == Some(true)
    {
        // Zero headroom: any new chunk pushes the projected total over the limit.
        policy.workspace_limit = usage_before.workspace_total_bytes;
    } else if let Some(x) = set.get("workspace_limit").and_then(Value::as_u64) {
        policy.workspace_limit = x;
    }
    store.set_quota_policy(&policy).unwrap();

    let write = &a["write"];
    let result = store.apply_mutation_crdt(&insert_mutation(&collection, write), &idx);
    let expect = &a["expect"];

    if expect["accepted"].as_bool() == Some(true) {
        result.unwrap_or_else(|e| panic!("{case}: write must be accepted, got {e}"));
        let want = &expect["record_present"];
        let id = want["id"].as_str().unwrap();
        let env = store.get_record(&collection, id).unwrap().unwrap();
        for (k, v) in want["fields"].as_object().unwrap() {
            assert_eq!(env.fields.get(k), Some(v), "{case}: accepted record field {k}");
        }
        // Review 176 P1: the gate charges the same slices `quota_usage` reports, so an
        // ACCEPTED write never leaves the workspace over the limit it was checked
        // against.
        if expect["usage_within_workspace_limit_after_accept"].as_bool() == Some(true) {
            assert!(
                store.quota_usage().unwrap().workspace_total_bytes <= policy.workspace_limit,
                "{case}: an accepted write must not leave the workspace over its limit"
            );
        }
        return;
    }

    // Rejection path: typed error, prior data intact, rejected record absent.
    let err = result.expect_err(&format!("{case}: write must be rejected"));
    if let Some(code) = expect["error_code"].as_str() {
        assert_eq!(err.code(), code, "{case}: error code");
    }
    if let Some(contains) = expect["error_contains"].as_str() {
        assert!(
            format!("{err}").contains(contains),
            "{case}: error must contain {contains:?}, got {err}"
        );
    }
    if let Some(intact) = expect.get("record_intact") {
        let id = intact["id"].as_str().unwrap();
        let env = store.get_record(&collection, id).unwrap().unwrap();
        for (k, v) in intact["fields"].as_object().unwrap() {
            assert_eq!(env.fields.get(k), Some(v), "{case}: prior record field {k} intact");
        }
    }
    if let Some(absent) = expect["rejected_record_absent"].as_str() {
        assert!(
            store.get_record(&collection, absent).unwrap().is_none(),
            "{case}: the rejected record must never land"
        );
    }
    if expect["no_new_chunk"].as_bool() == Some(true) {
        let chunks_after: Vec<String> = store
            .get_chunks(&doc_id)
            .unwrap()
            .into_iter()
            .map(|c| c.chunk_id)
            .collect();
        assert_eq!(chunks_after, chunks_before, "{case}: no new chunk after a rejected write");
    }
    if expect["usage_unchanged"].as_bool() == Some(true) {
        assert_eq!(
            store.quota_usage().unwrap(),
            usage_before,
            "{case}: a rejected write leaves accounted usage unchanged"
        );
    }
}

fn run_dedup(case: &str, fx: &Value) {
    let mut store = Store::open_in_memory().unwrap();
    let a = &fx["assert"];
    let puts = a["puts"].as_array().unwrap();

    let mut hashes: Vec<String> = Vec::new();
    let mut attachments_bytes: Vec<u64> = Vec::new();
    for put in puts {
        let bytes = put["bytes"].as_str().unwrap().as_bytes();
        let res = store.put_attachment(bytes).unwrap();
        assert_eq!(
            res.stored_new,
            put["expect_stored_new"].as_bool().unwrap(),
            "{case}: stored_new for bytes {:?}",
            put["bytes"]
        );
        assert_eq!(
            res.refcount,
            put["expect_refcount"].as_u64().unwrap(),
            "{case}: refcount"
        );
        hashes.push(res.content_hash);
        attachments_bytes.push(
            store
                .quota_usage()
                .unwrap()
                .category_bytes(QuotaCategory::Attachments),
        );
    }

    let expect = &a["expect"];
    if expect["first_and_second_same_hash"].as_bool() == Some(true) {
        assert_eq!(hashes[0], hashes[1], "{case}: identical bytes share a content hash");
    }
    if expect["third_distinct_hash"].as_bool() == Some(true) {
        assert_ne!(hashes[0], hashes[2], "{case}: distinct bytes get a distinct hash");
    }
    if expect["attachments_bytes_after_dedup_unchanged_between_puts_1_and_2"].as_bool()
        == Some(true)
    {
        assert_eq!(
            attachments_bytes[0], attachments_bytes[1],
            "{case}: a dedup hit adds no accounted bytes"
        );
    }
    if expect["attachments_bytes_increases_on_third"].as_bool() == Some(true) {
        assert!(
            attachments_bytes[2] > attachments_bytes[1],
            "{case}: a distinct blob adds accounted bytes"
        );
    }
}

fn run_report(case: &str, fx: &Value) {
    let mut store = Store::open_in_memory().unwrap();
    let idx = IndexManager::new();
    let seed = &fx["seed"];
    for r in seed["records"].as_array().unwrap() {
        let collection = r["collection"].as_str().unwrap().to_string();
        store
            .apply_mutation_crdt(&insert_mutation(&collection, r), &idx)
            .unwrap();
    }
    for blob in seed["attachments"].as_array().unwrap() {
        store.put_attachment(blob.as_str().unwrap().as_bytes()).unwrap();
    }

    let usage = store.quota_usage().unwrap();
    let expect = &fx["assert"]["expect"];

    let applets: Vec<&str> = usage.per_applet.iter().map(|a| a.applet.as_str()).collect();
    let want_applets: Vec<&str> = expect["applets_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(applets, want_applets, "{case}: per-applet report (sorted)");

    let categories: Vec<&str> = usage
        .per_category
        .iter()
        .map(|c| c.category.as_str())
        .collect();
    let want_categories: Vec<&str> = expect["categories_present"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(categories, want_categories, "{case}: per-category report shape");

    if expect["attachments_bytes_positive"].as_bool() == Some(true) {
        assert!(usage.category_bytes(QuotaCategory::Attachments) > 0, "{case}: attachments bytes");
    }
    if expect["retained_chunks_bytes_positive"].as_bool() == Some(true) {
        assert!(
            usage.category_bytes(QuotaCategory::RetainedChunks) > 0,
            "{case}: retained_chunks bytes"
        );
    }
    if expect["workspace_total_is_sum_of_applets_plus_categories"].as_bool() == Some(true) {
        let applets_total: u64 = usage.per_applet.iter().map(|a| a.collections_bytes).sum();
        let categories_total: u64 = usage.per_category.iter().map(|c| c.bytes).sum();
        assert_eq!(
            usage.workspace_total_bytes,
            applets_total + categories_total,
            "{case}: workspace total is the sum of every accounted slice"
        );
    }
    if expect["two_reads_byte_equal"].as_bool() == Some(true) {
        assert_eq!(usage, store.quota_usage().unwrap(), "{case}: deterministic report");
    }
}

fn run_policy(case: &str, fx: &Value) {
    let mut store = Store::open_in_memory().unwrap();
    let a = &fx["assert"];

    // No override ⇒ the const DEFAULT.
    let default = store.quota_policy().unwrap();
    assert_eq!(default, QuotaPolicy::DEFAULT, "{case}: default policy");
    let want_default = &a["expect_default"];
    assert_eq!(
        default.workspace_limit,
        want_default["workspace_limit"].as_u64().unwrap(),
        "{case}: default workspace_limit"
    );
    assert_eq!(
        default.per_applet_limit,
        want_default["per_applet_limit"].as_u64().unwrap(),
        "{case}: default per_applet_limit"
    );
    assert_eq!(
        default.approaching_threshold,
        want_default["approaching_threshold"].as_f64().unwrap(),
        "{case}: default threshold"
    );

    // A persisted override is trusted state, read back as the effective policy.
    let mut overridden = QuotaPolicy::DEFAULT;
    overridden.workspace_limit = a["override"]["workspace_limit"].as_u64().unwrap();
    store.set_quota_policy(&overridden).unwrap();
    assert_eq!(
        store.quota_policy().unwrap().workspace_limit,
        a["expect_after_override"]["workspace_limit"].as_u64().unwrap(),
        "{case}: override read back"
    );

    // An invalid override is rejected (config validation).
    let mut invalid = QuotaPolicy::DEFAULT;
    invalid.workspace_limit = a["invalid_override_rejected"]["workspace_limit"].as_u64().unwrap();
    assert!(store.set_quota_policy(&invalid).is_err(), "{case}: invalid override rejected");
}

#[test]
fn dl22_quota_fixtures() {
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
            "decision" => run_decision(name, &fx),
            "enforce" => run_enforce(name, &fx),
            "dedup" => run_dedup(name, &fx),
            "report" => run_report(name, &fx),
            "policy" => run_policy(name, &fx),
            other => panic!("{name}: unknown assert kind {other}"),
        }
        ran += 1;
    }
    assert_eq!(ran, declared, "every quota fixture must run");
}
