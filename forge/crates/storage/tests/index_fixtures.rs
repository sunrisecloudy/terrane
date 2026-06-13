//! Data-driven conformance over the Codex dynamic-index vectors
//! (`forge/fixtures/indexes/`, manifest `count = 6`).
//!
//! Each fixture seeds canonical `records`, registers its `indexes[]` definitions,
//! builds the physical structures (collection-scoped JSON1 expression indexes /
//! populated FTS5 shadow tables) from canonical records, runs the declared query
//! through the index-aware planner, and asserts the pinned `uses_index`,
//! `index_id`, `planner.full_scan` warnings, and ordered `result_rows`.
//!
//! The fixtures are load-bearing: a planner that hardcodes `uses_index`, resolves
//! a stable `field_id` to the wrong JSON path, forgets that a deprecated index is
//! not a candidate, or skips the FTS path fails here.
//!
//! prd-merged/02-data-layer-prd.md DL-5/DL-6/DL-15; spec
//! `forge/spec/dynamic-indexes.md`.

use forge_domain::RecordEnvelope;
use forge_storage::{
    FullScanReason, IndexDef, IndexManager, IndexState, PlannedQuery, Query, QueryResult, Store,
};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/indexes")
        .canonicalize()
        .expect("index fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

fn seed_records(store: &Store, fixture: &serde_json::Value) {
    for rec in fixture
        .get("records")
        .and_then(|r| r.as_array())
        .unwrap_or_else(|| panic!("fixture has no records"))
    {
        let env: RecordEnvelope =
            serde_json::from_value(rec.clone()).expect("record is a valid envelope");
        store.put_record(&env).expect("seed put_record");
    }
}

fn index_manager(fixture: &serde_json::Value) -> IndexManager {
    let mut mgr = IndexManager::new();
    for idx in fixture
        .get("indexes")
        .and_then(|i| i.as_array())
        .unwrap_or_else(|| panic!("fixture has no indexes"))
    {
        let def = IndexDef::from_fixture_value(idx).expect("parse index def");
        mgr.register(def);
    }
    mgr
}

fn query_of(fixture: &serde_json::Value) -> Query {
    let plan = fixture.get("query").expect("fixture query");
    Query::from_fixture_value(plan).expect("parse index-fixture query")
}

/// The expected ordered ids from `expected.result_rows[].entity_id`.
fn expected_ids(fixture: &serde_json::Value) -> Vec<String> {
    fixture
        .get("expected")
        .and_then(|e| e.get("result_rows"))
        .and_then(|r| r.as_array())
        .unwrap()
        .iter()
        .map(|r| r.get("entity_id").and_then(|i| i.as_str()).unwrap().to_string())
        .collect()
}

/// Assert the planner outcome against `expected` (uses_index / index_id /
/// warnings / ordered result rows). Shared by every vector.
fn assert_outcome(name: &str, fixture: &serde_json::Value, planned: &PlannedQuery) {
    let expected = fixture.get("expected").unwrap();

    // Ordered result rows.
    let want_ids = expected_ids(fixture);
    assert_eq!(planned.ids(), want_ids, "{name}: ordered result ids");
    // The rows really are RecordEnvelopes (not just ids), so the result type is
    // load-bearing: assert a field value round-trips for the first row.
    if let (QueryResult::Rows(rows), Some(first)) = (
        &planned.result,
        expected
            .get("result_rows")
            .and_then(|r| r.as_array())
            .and_then(|r| r.first()),
    ) {
        if let Some(want_fields) = first.get("fields").and_then(|f| f.as_object()) {
            let got = &rows[0].envelope.fields;
            for (k, v) in want_fields {
                assert_eq!(got.get(k), Some(v), "{name}: row 0 field '{k}'");
            }
        }
    }

    // uses_index.
    let want_uses = expected.get("uses_index").and_then(|u| u.as_bool()).unwrap();
    assert_eq!(planned.uses_index, want_uses, "{name}: uses_index");

    // index_id (present only when an index is used).
    if let Some(want_id) = expected.get("index_id").and_then(|i| i.as_str()) {
        assert_eq!(
            planned.index_id.as_deref(),
            Some(want_id),
            "{name}: index_id"
        );
    } else {
        assert!(
            planned.index_id.is_none(),
            "{name}: no index_id expected, got {:?}",
            planned.index_id
        );
    }

    // Warnings (the full_scan vectors pin code/collection/field_id/reason/rows).
    let want_warnings = expected
        .get("warnings")
        .and_then(|w| w.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        planned.warnings.len(),
        want_warnings.len(),
        "{name}: warning count"
    );
    for (got, want) in planned.warnings.iter().zip(&want_warnings) {
        assert_eq!(got.code, want.get("code").unwrap().as_str().unwrap(), "{name}: code");
        assert_eq!(
            got.collection,
            want.get("collection").unwrap().as_str().unwrap(),
            "{name}: warning collection"
        );
        assert_eq!(
            got.field_id.as_deref(),
            want.get("field_id").and_then(|f| f.as_str()),
            "{name}: warning field_id"
        );
        let want_reason = want.get("reason").unwrap().as_str().unwrap();
        assert_eq!(got.reason.code(), want_reason, "{name}: warning reason");
        if let Some(rows) = want.get("estimated_rows").and_then(|r| r.as_i64()) {
            assert_eq!(got.estimated_rows, Some(rows), "{name}: estimated_rows");
        }
    }
}

/// Standard vector: seed, register, build, query, assert.
fn run_index_case(name: &str) {
    let fx = load(name);
    let store = Store::open_in_memory().expect("store");
    seed_records(&store, &fx);
    let mgr = index_manager(&fx);
    store.build_indexes(&mgr).expect("build physical indexes");
    let q = query_of(&fx);
    let planned = store.query_planned(&q, &mgr).expect("planned query");
    assert_outcome(name, &fx, &planned);
}

#[test]
fn indexed_equality_uses_expression_index() {
    run_index_case("indexed_equality_uses_expression_index.json");
}

#[test]
fn indexed_range_uses_expression_index() {
    run_index_case("indexed_range_uses_expression_index.json");
}

#[test]
fn non_indexed_full_scan_warning() {
    run_index_case("non_indexed_full_scan_warning.json");
}

#[test]
fn fts_text_search_uses_shadow_table() {
    run_index_case("fts_text_search_uses_shadow_table.json");
}

/// review 041/042 finding 4: text_search is NOT a bypass — it composes with the
/// normal query pipeline. This vector combines an FTS MATCH with a `where`
/// filter and a `limit`, in FTS rank order; the planner must apply the filter to
/// the match set and bound it with limit while preserving rank order.
#[test]
fn fts_text_search_with_filter_and_limit() {
    run_index_case("fts_text_search_with_filter_and_limit.json");
}

#[test]
fn deprecated_index_no_longer_used() {
    run_index_case("deprecated_index_no_longer_used.json");
}

/// The rebuild vector pins the DL-6 lifecycle: an index proposed after records
/// already exist is NOT usable while `proposed`/`rebuilding`, and becomes usable
/// only after a rebuild from canonical records flips it to `active`. We walk the
/// phases explicitly to prove the planner honors the lifecycle, not just the
/// final state.
#[test]
fn rebuild_after_records_activates_index() {
    let name = "rebuild_after_records_activates_index.json";
    let fx = load(name);
    let store = Store::open_in_memory().expect("store");
    // Phase: records exist before the index is proposed.
    seed_records(&store, &fx);

    // The final definition is active; derive the (collection, field_id) it
    // covers so we can replay the lifecycle states.
    let final_def = IndexDef::from_fixture_value(&fx["indexes"][0]).unwrap();
    let (collection, field_id) = (final_def.collection.clone(), final_def.field_id.clone());
    let q = query_of(&fx);

    // proposed -> not usable; the query still returns correct rows via a scan.
    let mut mgr = IndexManager::new();
    mgr.register(
        IndexDef::new(
            &collection,
            &field_id,
            forge_storage::IndexKind::Expression,
            IndexState::Proposed,
        )
        .unwrap(),
    );
    store.build_indexes(&mgr).expect("build (proposed builds nothing)");
    let proposed = store.query_planned(&q, &mgr).expect("planned proposed");
    assert!(!proposed.uses_index, "proposed index must not be used");
    assert_eq!(proposed.ids(), expected_ids(&fx), "rows correct while proposed");

    // rebuilding -> still not usable.
    mgr.set_state(
        &collection,
        &field_id,
        forge_storage::IndexKind::Expression,
        IndexState::Rebuilding,
    );
    store.build_indexes(&mgr).expect("build (rebuilding still not active)");
    let rebuilding = store.query_planned(&q, &mgr).expect("planned rebuilding");
    assert!(!rebuilding.uses_index, "rebuilding index must not be used");

    // active -> usable; rebuild from canonical records, then the planner uses it.
    mgr.set_state(
        &collection,
        &field_id,
        forge_storage::IndexKind::Expression,
        IndexState::Active,
    );
    store.build_indexes(&mgr).expect("rebuild from canonical records -> active");
    let active = store.query_planned(&q, &mgr).expect("planned active");
    assert_outcome(name, &fx, &active);
    // And the active result equals the reference full scan (records canonical).
    assert_eq!(
        active.ids(),
        proposed.ids(),
        "active index result equals the reference full scan"
    );
}

/// The deprecated vector specifically warns with `index_deprecated`, distinct
/// from the `no_index` case — pin that the reason discriminates.
#[test]
fn deprecated_reason_is_distinct_from_no_index() {
    let fx = load("deprecated_index_no_longer_used.json");
    let store = Store::open_in_memory().expect("store");
    seed_records(&store, &fx);
    let mgr = index_manager(&fx);
    store.build_indexes(&mgr).unwrap();
    let planned = store.query_planned(&query_of(&fx), &mgr).unwrap();
    assert_eq!(planned.warnings.len(), 1);
    assert_eq!(planned.warnings[0].reason, FullScanReason::IndexDeprecated);
}

/// The expression-index DDL is real, not just metadata: after building, the
/// physical index exists in `sqlite_master` and SQLite's own query planner
/// consults it for the equality predicate (EXPLAIN QUERY PLAN names it). This
/// pins that `uses_index = true` reflects a structure SQLite would actually use,
/// not a hardcoded flag.
#[test]
fn expression_index_is_physically_present_and_consulted() {
    let fx = load("indexed_equality_uses_expression_index.json");
    let store = Store::open_in_memory().expect("store");
    seed_records(&store, &fx);
    let mgr = index_manager(&fx);
    store.build_indexes(&mgr).unwrap();

    let conn = store.connection();
    // The index physically exists.
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_records_tasks_f_alice_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "expression index must be created");

    // SQLite's planner consults it for the indexed equality predicate. The path
    // is double-quoted at the leaf key (matching the index DDL); SQLite only
    // consults the expression index when the query expression is byte-identical.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM records \
             WHERE collection = 'tasks' AND json_extract(data, '$.field_ids.\"f_alice_1\"') = 'open'",
            [],
            |r| r.get::<_, String>(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_records_tasks_f_alice_1"),
        "SQLite must use the expression index, got plan: {plan}"
    );
}

/// The manifest's `count` is load-bearing: every declared case must have a
/// fixture file and be exercised here. If Codex adds a vector, this fails until
/// it is wired.
#[test]
fn manifest_lists_every_wired_case() {
    let manifest = load("manifest.json");
    let declared = manifest.get("count").and_then(|c| c.as_i64()).unwrap();
    let cases = manifest.get("cases").and_then(|c| c.as_array()).unwrap();
    assert_eq!(declared as usize, cases.len(), "manifest count vs case list");
    assert_eq!(declared, 7, "expected 7 dynamic-index vectors");
    for case in cases {
        let file = case.get("file").and_then(|f| f.as_str()).unwrap();
        let _ = load(file); // panics if missing/unparseable
    }
}
