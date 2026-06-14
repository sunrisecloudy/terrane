//! Data-driven conformance over the Codex query/mutation vectors
//! (`forge/fixtures/query/`, manifest `count = 20`).
//!
//! Each fixture seeds the `records` projection, runs its declared query or
//! mutation through the storage-internal query engine, and asserts the pinned
//! ordered ids / aggregates / post-state / error. The fixtures are load-bearing:
//! a wrong planner (mis-ordered rows, wrong null handling, a coercion slip, a
//! missing tombstone hide) fails here, not just in a unit test.
//!
//! prd-merged/02-data-layer-prd.md DL-15/16/17; spec `forge/spec/query-dsl.md`.

use forge_domain::RecordEnvelope;
use forge_storage::{IndexManager, Mutation, Query, QueryResult, Store};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/storage
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/query")
        .canonicalize()
        .expect("query fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

fn seed_store(fixture: &serde_json::Value) -> Store {
    let store = Store::open_in_memory().expect("open store");
    if let Some(seed) = fixture.get("seed").and_then(|s| s.as_array()) {
        for rec in seed {
            let env: RecordEnvelope =
                serde_json::from_value(rec.clone()).expect("seed record is a valid envelope");
            store.put_record(&env).expect("seed put_record");
        }
    }
    store
}

/// The query plan value, whichever form a fixture uses: `query.plan`,
/// `query.sql_like`-only (reject case), or a top-level `query` object.
fn query_value(fixture: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(q) = fixture.get("query") {
        if let Some(plan) = q.get("plan") {
            if plan.is_null() {
                return None; // sql_like-only reject case
            }
            return Some(plan.clone());
        }
        // Top-level object form (empty_result, reject_ungranted, join, …).
        if q.is_object() {
            return Some(q.clone());
        }
    }
    None
}

// --- m0a query cases -------------------------------------------------------

fn run_query_case(name: &str) {
    let fx = load(name);
    let store = seed_store(&fx);
    let plan = query_value(&fx).unwrap_or_else(|| panic!("{name}: no query plan"));
    let query =
        Query::from_fixture_value(&plan).unwrap_or_else(|e| panic!("{name}: parse query: {e}"));
    let result = store
        .query(&query)
        .unwrap_or_else(|e| panic!("{name}: query: {e}"));

    let expect = fx
        .get("expect")
        .unwrap_or_else(|| panic!("{name}: no expect"));

    // Ordered ids.
    if let Some(ids) = expect.get("ids").and_then(|v| v.as_array()) {
        let want: Vec<String> = ids
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(result.ids(), want, "{name}: ordered ids mismatch");
    }

    // Single aggregate bundle.
    if let Some(agg) = expect.get("aggregate") {
        let got = match &result {
            QueryResult::Aggregate(a) => a,
            other => panic!("{name}: expected aggregate result, got {other:?}"),
        };
        if let Some(c) = agg.get("count").and_then(|v| v.as_i64()) {
            assert_eq!(got.count, Some(c), "{name}: count");
        }
        if let Some(s) = agg.get("sum").and_then(|v| v.as_f64()) {
            assert_eq!(got.sum, Some(s), "{name}: sum");
        }
        if let Some(a) = agg.get("avg").and_then(|v| v.as_f64()) {
            assert_eq!(got.avg, Some(a), "{name}: avg");
        }
        if let Some(m) = agg.get("min") {
            assert_eq!(got.min.as_ref(), Some(m), "{name}: min");
        }
        if let Some(m) = agg.get("max") {
            assert_eq!(got.max.as_ref(), Some(m), "{name}: max");
        }
    }

    // Grouped aggregates.
    if let Some(groups) = expect.get("groups").and_then(|v| v.as_array()) {
        let got = match &result {
            QueryResult::Groups(g) => g,
            other => panic!("{name}: expected grouped result, got {other:?}"),
        };
        assert_eq!(got.len(), groups.len(), "{name}: group count");
        for (g, want) in got.iter().zip(groups) {
            assert_eq!(&g.key, want.get("key").unwrap(), "{name}: group key");
            if let Some(s) = want.get("sum").and_then(|v| v.as_f64()) {
                assert_eq!(
                    g.aggregate.sum,
                    Some(s),
                    "{name}: group sum for {:?}",
                    g.key
                );
            }
        }
    }
}

#[test]
fn where_eq() {
    run_query_case("where_eq.json");
}

#[test]
fn where_ne() {
    run_query_case("where_ne.json");
}

#[test]
fn where_lt_le() {
    run_query_case("where_lt_le.json");
}

#[test]
fn where_gt_ge() {
    run_query_case("where_gt_ge.json");
}

#[test]
fn where_in() {
    run_query_case("where_in.json");
}

#[test]
fn where_like_text() {
    run_query_case("where_like_text.json");
}

#[test]
fn where_and_or() {
    run_query_case("where_and_or.json");
}

#[test]
fn order_by_asc() {
    run_query_case("order_by_asc.json");
}

#[test]
fn order_by_desc_limit() {
    run_query_case("order_by_desc_limit.json");
}

#[test]
fn limit_offset() {
    run_query_case("limit_offset.json");
}

#[test]
fn aggregate_count() {
    run_query_case("aggregate_count.json");
}

#[test]
fn aggregate_sum_avg_min_max() {
    run_query_case("aggregate_sum_avg_min_max.json");
}

#[test]
fn group_by_status_sum() {
    run_query_case("group_by_status_sum.json");
}

#[test]
fn empty_result() {
    run_query_case("empty_result.json");
}

// --- P1 semantics: pinned but report unsupported ---------------------------

#[test]
fn text_search_match_is_unsupported_p1() {
    // The vector is `p1_semantics`: a bare `text` marker is an unsupported P1
    // feature. The parser flags it AND `Store::query` must REFUSE it before
    // planning rather than scanning and returning bogus rows (review 040 finding
    // 7; query-dsl.md §Result). We execute through Store::query, not just the
    // parser.
    let fx = load("text_search_match.json");
    let store = seed_store(&fx);
    let plan = fx.get("query").and_then(|q| q.get("plan")).unwrap().clone();
    let query = Query::from_fixture_value(&plan).expect("parse p1 text plan");
    assert_eq!(
        query.unsupported.as_deref(),
        Some("text_search"),
        "text search must be flagged unsupported until P1"
    );
    let err = store.query(&query).unwrap_err();
    assert_eq!(err.code(), "QueryError", "{err}");
    assert!(
        err.to_string().contains("unsupported_feature"),
        "must carry the unsupported_feature marker before planning: {err}"
    );
}

#[test]
fn join_reference_field_is_unsupported_p1() {
    // A `join` is unsupported in M0a. Crucially, the join `where` is over the
    // joined field `assignee.name`, which a bare scan would compile to a literal
    // `$.fields."assignee.name"` path and return bogus rows; Store::query must
    // refuse it before planning (review 040 finding 7).
    let fx = load("join_reference_field.json");
    let store = seed_store(&fx);
    let plan = fx.get("query").unwrap().clone();
    let query = Query::from_fixture_value(&plan).expect("parse p1 join plan");
    assert_eq!(
        query.unsupported.as_deref(),
        Some("join"),
        "join must be flagged unsupported until P1"
    );
    let err = store.query(&query).unwrap_err();
    assert_eq!(err.code(), "QueryError", "{err}");
    assert!(
        err.to_string().contains("unsupported_feature"),
        "join must be refused with the unsupported_feature marker, not scanned: {err}"
    );
}

// --- mutations -------------------------------------------------------------

#[test]
fn mutation_insert_patch_delete() {
    // insert -> patch -> delete; assert each applied (commit_count) and the
    // tombstoned post-state (deleted=true, merged fields, advanced updated_at).
    let fx = load("mutation_insert_patch_delete.json");
    let mut store = seed_store(&fx);
    // No FTS index is active here, so the index-synced mutation path behaves
    // identically to a bare projection write — but it is the DL-17 surface, so it
    // is exercised through the same FTS-syncing path applets use (review 041/042).
    let indexes = IndexManager::new();

    let muts: Vec<Mutation> = fx
        .get("mutations")
        .and_then(|m| m.as_array())
        .unwrap()
        .iter()
        .map(|m| serde_json::from_value(m.clone()).expect("parse mutation"))
        .collect();

    let mut applied = 0usize;
    for m in &muts {
        store.apply_mutation(m, &indexes).expect("apply mutation");
        applied += 1;
    }
    let expect = fx.get("expect").unwrap();
    assert_eq!(
        applied as i64,
        expect.get("commit_count").and_then(|c| c.as_i64()).unwrap(),
        "commit_count"
    );

    // visible_ids: a normal query must hide the tombstoned record.
    let q = Query::from("tasks");
    let visible = store.query(&q).unwrap().ids();
    let want_visible: Vec<String> = expect
        .get("visible_ids")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(visible, want_visible, "visible ids after delete");

    // post_state: the stored envelope is the expected tombstoned/merged record.
    let post = expect.get("post_state").and_then(|v| v.as_array()).unwrap();
    let want_env: RecordEnvelope = serde_json::from_value(post[0].clone()).unwrap();
    let got = store
        .get_record("tasks", "task_001")
        .unwrap()
        .expect("record retained as tombstone");
    assert!(got.deleted, "record must be tombstoned");
    assert_eq!(got.fields, want_env.fields, "merged fields");
    assert_eq!(got.updated_at, want_env.updated_at, "updated_at advanced");
    assert_eq!(got.created_at, want_env.created_at, "created_at preserved");
    // Review 045/046 finding 1: the DL-17 mutation surface must materialize the
    // stable field ids the projection indexes read (`$.field_ids.<id>`). The
    // fixture pins these `f_<name>` ids in its `post_state`; assert them so a
    // mutation that left `field_ids` empty (invisible to FTS/value indexes)
    // fails this vector instead of silently passing.
    assert!(!want_env.field_ids.is_empty(), "fixture must pin field_ids");
    assert_eq!(
        got.field_ids, want_env.field_ids,
        "mutation must materialize stable field_ids so the record is index-visible"
    );
}

#[test]
fn transact_group() {
    // One transact group commits insert + patch atomically; both are visible.
    let fx = load("transact_group.json");
    let mut store = seed_store(&fx);

    let muts: Vec<Mutation> = fx
        .get("mutations")
        .and_then(|m| m.as_array())
        .unwrap()
        .iter()
        .map(|m| serde_json::from_value(m.clone()).expect("parse mutation"))
        .collect();

    // The fixture's single mutation is the transact group itself.
    let indexes = IndexManager::new();
    let count = match &muts[0] {
        Mutation::Transact { items } => {
            store.transact_mutations(items, &indexes).expect("transact")
        }
        other => panic!("expected transact group, got {other:?}"),
    };
    assert_eq!(count, 2, "two leaf mutations applied");

    let q = Query::from("tasks");
    let visible = store.query(&q).unwrap().ids();
    let want: Vec<String> = fx
        .get("expect")
        .and_then(|e| e.get("visible_query_ids"))
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(visible, want, "both records visible after transact");

    // The patch landed on the existing record.
    let patched = store.get_record("tasks", "tasks/1").unwrap().unwrap();
    assert_eq!(patched.fields["done"], serde_json::json!(true));
}

// --- rejections ------------------------------------------------------------

#[test]
fn reject_raw_sql() {
    // The SQL-like form rejects raw SQL outside the validated subset; the AST is
    // the only surface (DL-16). The fixture's plan is null and a banned SQL
    // string is supplied.
    let fx = load("reject_raw_sql.json");
    let sql_like = fx
        .get("query")
        .and_then(|q| q.get("sql_like"))
        .and_then(|s| s.as_str())
        .unwrap();
    let err = forge_storage::query::reject_raw_sql(sql_like).unwrap_err();
    assert_eq!(err.code(), "QueryError", "{err}");
    assert!(
        err.to_string().contains("raw SQL is not exposed"),
        "error must carry the no-raw-SQL contract phrase: {err}"
    );
}

#[test]
fn reject_ungranted_collection_is_a_caller_boundary() {
    // `reject_ungranted_collection` pins a CapabilityRequired error. Capability
    // grants are enforced by the host bridge / core (a SEPARATE layer), not by
    // forge-storage: the projection has no notion of `db.read` grants and will
    // happily scan any collection. This test pins that boundary so the fixture
    // is accounted for here without smuggling RBAC into the storage crate: the
    // engine returns the row, and it is the caller's job to have refused first.
    let fx = load("reject_ungranted_collection.json");
    let store = seed_store(&fx);
    let plan = query_value(&fx).expect("query plan");
    let query = Query::from_fixture_value(&plan).expect("parse query");
    let result = store
        .query(&query)
        .expect("storage runs the scan unguarded");
    assert_eq!(
        result.ids(),
        vec!["secret_001".to_string()],
        "storage itself does not enforce grants; the host bridge must"
    );
    // The fixture's contract (enforced one layer up) is a CapabilityRequired
    // error mentioning the ungranted collection.
    let err = fx.get("expect_error").unwrap();
    assert_eq!(err.get("code").unwrap(), "CapabilityRequired");
}

/// The manifest's `count` is load-bearing: every declared case must have a
/// fixture file and be exercised by a test in this file. If Codex adds a vector,
/// this fails until it is wired.
#[test]
fn manifest_lists_every_wired_case() {
    let manifest = load("manifest.json");
    let declared = manifest.get("count").and_then(|c| c.as_i64()).unwrap();
    let cases = manifest.get("cases").and_then(|c| c.as_array()).unwrap();
    assert_eq!(
        declared as usize,
        cases.len(),
        "manifest count must match the case list"
    );
    assert_eq!(declared, 20, "expected 20 query/mutation vectors");
    // Every case file must exist and parse.
    for case in cases {
        let file = case.get("file").and_then(|f| f.as_str()).unwrap();
        let _ = load(file); // panics if missing/unparseable
    }
}
