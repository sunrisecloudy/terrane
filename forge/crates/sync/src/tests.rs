//! Tests for the in-process CRDT sync seam (SS-1/SS-2, M0b).
//!
//! Two layers:
//!
//! 1. A **data-driven** suite over `forge/fixtures/sync/*.json` (the Codex T026
//!    convergence corpus). For each case we build peer A's and peer B's
//!    [`Store`]s — each under its own distinct Loro peer id — apply the seed and
//!    the per-peer divergent ops, run [`sync_stores`], and assert BOTH peers'
//!    record projections equal the fixture's `expect_converged` in every
//!    collection. The fixtures are load-bearing: a broken chunk diff or merge
//!    leaves a peer short a record (or with the wrong field) and the assertion
//!    fails.
//!
//! 2. **Unit tests** pinning the SS-2 observable invariants the fixtures imply
//!    but do not isolate: a second sync moves zero chunks (idempotent), a
//!    one-directional catch-up, and an empty-peer catch-up.

use super::*;
use forge_storage::{collection_doc_id, CreateIndexKind, IndexManager, Mutation, Store};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

// ---------------------------------------------------------------- fixture model

/// One op inside a fixture's `seed` / `peer_a` / `peer_b` list.
#[derive(serde::Deserialize)]
struct FixtureOp {
    op: String,
    collection: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    fields: Map<String, Value>,
}

/// One expected converged record (`{id, fields}`) in a collection.
#[derive(serde::Deserialize)]
struct ExpectRecord {
    id: String,
    fields: Value,
}

/// A T026 convergence fixture (`fixtures/sync/<case>.json`).
#[derive(serde::Deserialize)]
struct Fixture {
    case: String,
    peer_a_id: u64,
    peer_b_id: u64,
    #[serde(default)]
    seed: Vec<FixtureOp>,
    #[serde(default)]
    seed_mode: Option<String>,
    #[serde(default)]
    peer_a: Vec<FixtureOp>,
    #[serde(default)]
    peer_b: Vec<FixtureOp>,
    /// collection -> expected converged records. For an ambiguous-winner case a
    /// field value may be an `{one_of, agreement_required}` marker instead of a
    /// scalar; those fields are checked for peer agreement only (see below).
    expect_converged: BTreeMap<String, Vec<ExpectRecord>>,
    #[serde(default)]
    expect_deleted_ids: Vec<String>,
}

/// Turn a fixture op into a storage [`Mutation`], threading a monotone logical
/// clock so timestamps advance deterministically.
fn op_to_mutation(op: &FixtureOp, at: i64) -> Mutation {
    match op.op.as_str() {
        "insert" => Mutation::Insert {
            collection: op.collection.clone(),
            id: Some(op.id.clone().expect("insert op needs an id")),
            fields: op.fields.clone(),
            logical_at: Some(at),
        },
        "patch" => Mutation::Patch {
            collection: op.collection.clone(),
            id: op.id.clone().expect("patch op needs an id"),
            fields: op.fields.clone(),
            logical_at: Some(at),
        },
        "delete" => Mutation::Delete {
            collection: op.collection.clone(),
            id: op.id.clone().expect("delete op needs an id"),
            logical_at: Some(at),
        },
        other => panic!("unknown fixture op {other}"),
    }
}

/// Apply each op in `ops` to `store` as one DL-4 CRDT logical write, advancing a
/// shared clock so ordering is deterministic across peers.
fn apply_ops(store: &mut Store, ops: &[FixtureOp], idx: &IndexManager, clock: &mut i64) {
    for op in ops {
        *clock += 1;
        let m = op_to_mutation(op, *clock);
        store.apply_mutation_crdt(&m, idx).expect("apply op");
    }
}

/// Copy every persisted chunk of `from` verbatim (same `doc_id`/`chunk_id`/
/// `format`/`payload`) into `into`. Used to clone a single baseline CRDT history
/// into both peers for a `seed_mode: "shared_history"` fixture, so the seed is
/// genuinely *shared* history (byte-identical chunks) rather than two independent
/// inserts that would conflict.
fn copy_chunks(into: &Store, from: &Store) {
    for doc_id in from.list_doc_ids().expect("list seed doc ids") {
        for chunk in from.get_chunks(&doc_id).expect("read seed chunks") {
            into.put_chunk(&doc_id, &chunk.chunk_id, &chunk.format, &chunk.payload)
                .expect("seed chunk into peer");
        }
    }
}

/// The visible projection of a store as `collection -> {id -> fields}` (deleted
/// records excluded), for converged-state comparison.
fn projection(store: &Store, collections: &[&str]) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for &collection in collections {
        let mut by_id = BTreeMap::new();
        for env in store.list_records(collection).expect("list records") {
            if env.deleted {
                continue;
            }
            by_id.insert(
                env.entity_id.as_str().to_string(),
                serde_json::to_value(&env.fields).expect("fields to json"),
            );
        }
        out.insert(collection.to_string(), by_id);
    }
    out
}

/// True iff `value` is an ambiguous-winner marker (`{one_of: [...],
/// agreement_required: true}`) rather than a concrete scalar. Such a field's
/// exact value is implementation-defined (Loro LWW tie-break), so the fixture
/// only requires the two peers to AGREE and the value to be one of the listed
/// options.
fn is_ambiguous(value: &Value) -> bool {
    value
        .as_object()
        .map(|o| o.contains_key("one_of") && o.contains_key("agreement_required"))
        .unwrap_or(false)
}

/// Assert one store's projection matches `expect_converged`, treating an
/// ambiguous-winner field as "must be one of the listed options" (peer agreement
/// is asserted separately by comparing both peers' full projections).
fn assert_matches_expected(
    actual: &BTreeMap<String, BTreeMap<String, Value>>,
    expect: &BTreeMap<String, Vec<ExpectRecord>>,
    case: &str,
) {
    for (collection, records) in expect {
        let got = actual
            .get(collection)
            .unwrap_or_else(|| panic!("case {case}: missing collection {collection}"));
        assert_eq!(
            got.len(),
            records.len(),
            "case {case}: collection {collection} record count mismatch (got {got:?})"
        );
        for rec in records {
            let got_fields = got
                .get(&rec.id)
                .unwrap_or_else(|| panic!("case {case}: missing record {collection}/{}", rec.id));
            let expect_fields = rec.fields.as_object().expect("expected fields object");
            assert_eq!(
                got_fields.as_object().expect("got fields object").len(),
                expect_fields.len(),
                "case {case}: {collection}/{} field count mismatch (got {got_fields:?})",
                rec.id
            );
            for (key, want) in expect_fields {
                let have = got_fields
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case}: {collection}/{} missing field {key}", rec.id));
                if is_ambiguous(want) {
                    let options = want["one_of"].as_array().expect("one_of array");
                    assert!(
                        options.contains(have),
                        "case {case}: {collection}/{} field {key} = {have} not in {options:?}",
                        rec.id
                    );
                } else {
                    assert_eq!(
                        have, want,
                        "case {case}: {collection}/{} field {key} mismatch",
                        rec.id
                    );
                }
            }
        }
    }
}

/// Run one fixture end to end: build both peers (distinct peer ids), apply the
/// seed (cloned as shared history when requested) and divergent ops, sync, then
/// assert BOTH peers converged to `expect_converged` and AGREE with each other.
fn run_fixture(path: &std::path::Path) {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    let fx: Fixture = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()));

    assert_ne!(
        fx.peer_a_id, fx.peer_b_id,
        "case {}: the two peers must use distinct Loro peer ids",
        fx.case
    );

    let idx = IndexManager::new();
    let mut peer_a = Store::open_in_memory().unwrap().with_crdt_peer_id(fx.peer_a_id);
    let mut peer_b = Store::open_in_memory().unwrap().with_crdt_peer_id(fx.peer_b_id);

    // A shared clock across seed + both peers so logical timestamps are
    // deterministic and the divergent ops happen "after" the seed.
    let mut clock = 0i64;

    let shared_history = fx.seed_mode.as_deref() == Some("shared_history");
    if !fx.seed.is_empty() {
        if shared_history {
            // Build ONE baseline history and clone its chunks byte-for-byte into
            // both peers, so the seed is genuine shared history (not two
            // independent inserts that would conflict).
            let mut seed = Store::open_in_memory().unwrap().with_crdt_peer_id(fx.peer_a_id);
            apply_ops(&mut seed, &fx.seed, &idx, &mut clock);
            copy_chunks(&peer_a, &seed);
            copy_chunks(&peer_b, &seed);
            peer_a.rebuild_projection(&idx).unwrap();
            peer_b.rebuild_projection(&idx).unwrap();
        } else {
            // No shared-history hint: each peer applies the seed independently.
            let mut a_clock = clock;
            apply_ops(&mut peer_a, &fx.seed, &idx, &mut a_clock);
            apply_ops(&mut peer_b, &fx.seed, &idx, &mut clock);
        }
    }

    // The divergent per-peer ops, each under its own peer id.
    apply_ops(&mut peer_a, &fx.peer_a, &idx, &mut clock);
    apply_ops(&mut peer_b, &fx.peer_b, &idx, &mut clock);

    // Converge the two workspaces. Both peers share the same (empty) index set in
    // these fixtures, so each rebuilds against `idx` — passed per store now.
    let report = sync_stores(&mut peer_a, &idx, &mut peer_b, &idx).unwrap();

    // A second sync must be a no-op (idempotent): the frontiers now match.
    let again = sync_stores(&mut peer_a, &idx, &mut peer_b, &idx).unwrap();
    assert_eq!(
        again.total_chunks_moved(),
        0,
        "case {}: a second sync moved chunks ({again:?}) — sync is not idempotent",
        fx.case
    );

    let collections: Vec<&str> = fx.expect_converged.keys().map(|s| s.as_str()).collect();
    let proj_a = projection(&peer_a, &collections);
    let proj_b = projection(&peer_b, &collections);

    // Both peers agree (identical projection) — this is the assertion that covers
    // the ambiguous-winner case where the exact value is implementation-defined.
    assert_eq!(
        proj_a, proj_b,
        "case {}: peers disagree after sync (report {report:?})",
        fx.case
    );

    // Each peer matches the declared converged state (ambiguous fields checked as
    // "one of" only).
    assert_matches_expected(&proj_a, &fx.expect_converged, &fx.case);
    assert_matches_expected(&proj_b, &fx.expect_converged, &fx.case);

    // Deleted ids (e.g. `tasks/t1`) must be absent from BOTH peers' visible set.
    for deleted in &fx.expect_deleted_ids {
        let (collection, id) = deleted
            .split_once('/')
            .unwrap_or_else(|| panic!("case {}: malformed deleted id {deleted}", fx.case));
        assert!(
            !proj_a.get(collection).map(|c| c.contains_key(id)).unwrap_or(false),
            "case {}: deleted id {deleted} still visible on peer A",
            fx.case
        );
        assert!(
            !proj_b.get(collection).map(|c| c.contains_key(id)).unwrap_or(false),
            "case {}: deleted id {deleted} still visible on peer B",
            fx.case
        );
    }
}

/// The `forge/fixtures/sync` directory (relative to this crate).
fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sync")
}

#[test]
fn every_sync_fixture_converges() {
    let dir = fixtures_dir();
    let mut ran = 0usize;
    for entry in std::fs::read_dir(&dir).expect("read fixtures/sync dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            continue;
        }
        run_fixture(&path);
        ran += 1;
    }
    // The T026 corpus is 10 convergence cases (+ the manifest). Guard against a
    // silently empty run (e.g. a moved fixtures dir) so the suite stays
    // load-bearing.
    assert_eq!(ran, 10, "expected 10 sync fixtures, ran {ran}");
}

// ---------------------------------------------------------------- unit tests

/// A minimal insert mutation for the unit tests.
fn insert(collection: &str, id: &str, fields: Value, at: i64) -> Mutation {
    Mutation::Insert {
        collection: collection.into(),
        id: Some(id.into()),
        fields: fields.as_object().expect("object").clone(),
        logical_at: Some(at),
    }
}

#[test]
fn second_sync_moves_no_chunks() {
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    a.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "a"}), 1), &idx)
        .unwrap();
    b.apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "b"}), 2), &idx)
        .unwrap();

    let first = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert!(first.total_chunks_moved() > 0, "first sync should move chunks");

    let second = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert_eq!(second.total_chunks_moved(), 0, "second sync must be a no-op");
}

#[test]
fn authorized_gate_skips_denied_chunks_and_carries_allowed_with_envelope() {
    // The SS-7 mechanism at the sync seam: each staged op is authorized BEFORE
    // import. A `false` decision drops the chunk (nothing lands in the receiver);
    // a `true` decision imports it. The envelope handed to the gate carries the op
    // + collection recovered from the ORIGIN oplog / doc id.
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    // a authors a `tasks` insert and a `notes` insert.
    a.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "x"}), 1), &idx)
        .unwrap();
    a.apply_mutation_crdt(&insert("notes", "n1", json!({"body": "y"}), 2), &idx)
        .unwrap();

    // Authorize only the `tasks` collection; deny `notes`. Capture the envelopes
    // the gate observed to assert the op + collection were recovered.
    let mut seen: Vec<(String, SyncRecordOp, String)> = Vec::new();
    let report = sync_stores_authorized(&mut a, &idx, &mut b, &idx, |source, env| {
        seen.push((source.to_string(), env.op, env.collection.clone()));
        env.collection == "tasks"
    })
    .unwrap();

    // One denial (notes), one import (tasks).
    assert_eq!(report.chunks_denied, 1, "the notes op was denied");
    assert_eq!(report.chunks_a_to_b, 1, "only the tasks op imported into b");

    // b got the tasks record and NOT the notes record (the denied chunk was
    // skipped before import — the receiver's history is unchanged for it).
    assert_eq!(
        b.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
        json!("x")
    );
    assert!(b.get_record("notes", "n1").unwrap().is_none(), "notes op skipped");
    assert!(
        b.get_chunks(&collection_doc_id("notes")).unwrap().is_empty(),
        "no notes chunk landed in b"
    );

    // The gate saw both ops as inserts from a's source, with the right collections.
    seen.sort_by(|l, r| l.2.cmp(&r.2));
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].1, SyncRecordOp::Insert);
    assert_eq!(seen[0].2, "notes");
    assert_eq!(seen[1].2, "tasks");
    assert!(seen.iter().all(|(s, _, _)| s == "peer:11"));
}

#[test]
fn one_directional_catchup_brings_lagging_peer_current() {
    // a holds a write b lacks; a single sync_stores carries it b-ward.
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    a.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "only-on-a"}), 1), &idx)
        .unwrap();

    let report = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert_eq!(report.chunks_a_to_b, 1, "the one write should move a->b");
    assert_eq!(report.chunks_b_to_a, 0, "b had nothing to send");

    let env = b.get_record("tasks", "t1").unwrap().expect("b caught up");
    assert_eq!(env.fields["title"], json!("only-on-a"));
}

#[test]
fn empty_peer_catches_up_via_pull() {
    // The one-directional `pull` half: a fresh empty peer pulls all of a's docs.
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    a.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "x"}), 1), &idx)
        .unwrap();
    a.apply_mutation_crdt(&insert("notes", "n1", json!({"body": "y"}), 2), &idx)
        .unwrap();

    let mut empty = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    let moved = pull(&mut empty, &a, &idx).unwrap();
    assert_eq!(moved, 2, "the empty peer should import both docs' chunks");
    assert_eq!(
        empty.get_record("tasks", "t1").unwrap().unwrap().fields["title"],
        json!("x")
    );
    assert_eq!(
        empty.get_record("notes", "n1").unwrap().unwrap().fields["body"],
        json!("y")
    );

    // Re-pulling moves nothing (frontiers now match).
    assert_eq!(pull(&mut empty, &a, &idx).unwrap(), 0);
}

/// Review 084 #1 (P1): two peers with ASYMMETRIC active indexes must each rebuild
/// against their OWN [`IndexManager`], so `sync_stores` is NOT order-dependent.
///
/// Peer A holds an active FTS index on `notes/f_body`; peer B holds NONE. Before
/// the fix, `sync_stores` rebuilt BOTH stores with a single manager: rebuilding the
/// FTS-less store against A's manager issued FTS DML against a table that store
/// lacks (an error / corruption), and the reverse order left A's FTS index stale.
/// With per-store managers neither happens, in either order.
///
/// The assertions, for both `(a, b)` and `(b, a)` argument orders:
///   - sync returns `Ok` (no FTS-DML-against-missing-table error);
///   - the two stores' record projections are byte-identical (convergence);
///   - the FTS store's index is INTACT and queryable for BOTH peers' notes
///     (A's own seeded note AND B's note that arrived only via sync);
///   - the non-FTS store gained no FTS table.
fn build_fts_peer(peer_id: u64, seed_note: (&str, &str)) -> (Store, IndexManager) {
    let mut store = Store::open_in_memory().unwrap().with_crdt_peer_id(peer_id);
    let mut idx = IndexManager::new();
    // Seed one note so the FTS index has a live row, then activate the FTS index
    // over the materialized `f_body` field id (DL-5).
    store
        .apply_mutation_crdt(
            &insert("notes", seed_note.0, json!({ "body": seed_note.1 }), 1),
            &idx,
        )
        .unwrap();
    store
        .create_index(&mut idx, "notes", "f_body", CreateIndexKind::Fts)
        .expect("activate FTS index on notes/f_body");
    (store, idx)
}

fn build_plain_peer(peer_id: u64, seed_note: (&str, &str)) -> (Store, IndexManager) {
    let mut store = Store::open_in_memory().unwrap().with_crdt_peer_id(peer_id);
    let idx = IndexManager::new();
    store
        .apply_mutation_crdt(
            &insert("notes", seed_note.0, json!({ "body": seed_note.1 }), 2),
            &idx,
        )
        .unwrap();
    (store, idx)
}

/// Drive one sync direction and assert the post-sync invariants. `fts_first`
/// selects which argument slot the FTS peer occupies, proving order-independence.
fn assert_asymmetric_sync_converges(fts_first: bool) {
    // Peer A: FTS-indexed, seeded with a note containing "offline".
    let (mut a, a_idx) = build_fts_peer(101, ("a1", "offline sync keeps indexes honest"));
    // Peer B: NO indexes, seeded with a note containing "lunch".
    let (mut b, b_idx) = build_plain_peer(202, ("b1", "lunch plans for the whole team"));

    // Run sync in the requested argument order — each store paired with ITS OWN
    // index manager (the review 084 #1 contract).
    let report = if fts_first {
        sync_stores(&mut a, &a_idx, &mut b, &b_idx)
    } else {
        sync_stores(&mut b, &b_idx, &mut a, &a_idx)
    }
    .unwrap_or_else(|e| panic!("sync_stores(fts_first={fts_first}) errored: {e:?}"));
    assert!(
        report.total_chunks_moved() > 0,
        "fts_first={fts_first}: the two divergent notes should move"
    );

    // Convergence: both stores hold the identical visible projection.
    let proj_a = projection(&a, &["notes"]);
    let proj_b = projection(&b, &["notes"]);
    assert_eq!(
        proj_a, proj_b,
        "fts_first={fts_first}: peers disagree after asymmetric sync"
    );
    assert_eq!(
        proj_a["notes"].len(),
        2,
        "fts_first={fts_first}: both notes (a1 + b1) should be present"
    );

    // The FTS store's index is INTACT and queryable for BOTH notes — its own
    // seeded one AND b1, which arrived only through sync and was folded in by the
    // rebuild against A's OWN manager.
    assert_eq!(
        a_idx
            .fts_match(a.connection(), "notes", "f_body", "offline")
            .unwrap(),
        vec!["a1".to_string()],
        "fts_first={fts_first}: A's own note dropped out of its FTS index"
    );
    assert_eq!(
        a_idx
            .fts_match(a.connection(), "notes", "f_body", "lunch")
            .unwrap(),
        vec!["b1".to_string()],
        "fts_first={fts_first}: B's synced note was not indexed in A's FTS"
    );

    // The plain store never grew an FTS table: it rebuilt against its OWN (empty)
    // manager, so a search against its connection finds no such index.
    assert!(
        b_idx.get_fts("notes", "f_body").is_none(),
        "fts_first={fts_first}: the non-FTS peer must not have gained an FTS index"
    );
}

#[test]
fn asymmetric_indexes_sync_is_not_order_dependent() {
    // FTS peer as `a` (the original failing order: rebuild the FTS-less store).
    assert_asymmetric_sync_converges(true);
    // FTS peer as `b` (the reverse order: A's own FTS index must not go stale).
    assert_asymmetric_sync_converges(false);
}

/// Review 084 #2 (P2): a chunk that arrives via sync must be recorded in the
/// RECEIVING store's oplog (DL-4: "Remote updates follow the identical path"), so
/// the change-feed / audit surface sees remote imports — not just local writes.
/// A re-sync (idempotent) must add NO new oplog rows.
///
/// Setup: peer A inserts `t1`, peer B inserts `t2`; one `sync_stores` exchanges the
/// two chunks. Then assert, on EACH receiving store:
///   - the oplog now contains a row for the chunk it imported, tagged remote
///     (`kind == record.remote_import`, `actor_id`/`workspace_id` mark it remote,
///     payload carries the source peer id) and distinguishable from local ops;
///   - a second `sync_stores` (converged, idempotent) appends ZERO new oplog rows.
#[test]
fn synced_chunks_are_recorded_in_receiver_oplog_as_remote_and_idempotent() {
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    a.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "from-a"}), 1), &idx)
        .unwrap();
    b.apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "from-b"}), 2), &idx)
        .unwrap();

    // Before sync each store has exactly its one LOCAL op (kind record.insert).
    let a_local = a.list_ops().unwrap();
    let b_local = b.list_ops().unwrap();
    assert_eq!(a_local.len(), 1);
    assert_eq!(a_local[0].kind, "record.insert");
    assert_eq!(b_local.len(), 1);

    let first = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert_eq!(first.total_chunks_moved(), 2, "the two divergent chunks move");

    // Each store gained exactly ONE oplog row from the import: its prior local op
    // plus the one remote chunk it received.
    let a_ops = a.list_ops().unwrap();
    let b_ops = b.list_ops().unwrap();
    assert_eq!(a_ops.len(), 2, "A: local op + one imported remote op");
    assert_eq!(b_ops.len(), 2, "B: local op + one imported remote op");

    // The imported row is tagged remote and attributable to the SOURCE peer, and is
    // distinguishable from the local op (different kind / actor / workspace).
    let assert_remote_import = |ops: &[forge_storage::OpRow], source_peer: u64| {
        let remote: Vec<_> = ops
            .iter()
            .filter(|o| o.kind == "record.remote_import")
            .collect();
        assert_eq!(remote.len(), 1, "exactly one remote-import op expected");
        let op = remote[0];
        assert_eq!(op.workspace_id, "remote", "remote import marked remote");
        let expected_source = format!("peer:{source_peer}");
        assert_eq!(op.actor_id, expected_source, "actor tags the source peer");
        let payload: Value = serde_json::from_slice(&op.payload).unwrap();
        assert_eq!(payload["source"], json!(expected_source));
        assert_eq!(payload["kind"], json!("record.remote_import"));
        // The local op is NOT mistaken for a remote one.
        assert!(
            ops.iter().any(|o| o.kind == "record.insert"),
            "the local insert op must still be present and distinct"
        );
    };
    // A received B's chunk (source = peer:22); B received A's chunk (source peer:11).
    assert_remote_import(&a_ops, 22);
    assert_remote_import(&b_ops, 11);

    // Idempotence: a second sync over the now-converged pair moves no chunks AND
    // appends NO new oplog rows (re-importing a present chunk is a pure no-op).
    let second = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert_eq!(second.total_chunks_moved(), 0, "converged: no chunks move");
    assert_eq!(a.list_ops().unwrap().len(), 2, "A: no duplicate import op");
    assert_eq!(b.list_ops().unwrap().len(), 2, "B: no duplicate import op");
}
