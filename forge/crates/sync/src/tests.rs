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
    let report = sync_stores_authorized(&mut a, &idx, &mut b, &idx, |source, env, _audit| {
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
fn forwarded_chunk_envelope_carries_original_author_not_relay() {
    // review 092 #1: C authors a chunk, A imports it (A is only a RELAY), then A
    // stages it for B. The envelope A presents must carry C's ORIGINAL source in
    // `origin_source` (recovered from A's `record.remote_import` oplog provenance),
    // so the receiver gates the chunk against C — not the relay A — for SS-7 actor
    // identity. A chunk A authored locally has `origin_source == None`.
    let idx = IndexManager::new();
    let mut c = Store::open_in_memory().unwrap().with_crdt_peer_id(33); // original author
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11); // relay
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22); // receiver

    // C authors `tasks/c1`; A imports it (A is now only a relay for that chunk).
    c.apply_mutation_crdt(&insert("tasks", "c1", json!({"title": "from-c"}), 1), &idx)
        .unwrap();
    let pulled = pull(&mut a, &c, &idx).unwrap();
    assert_eq!(pulled, 1, "A imported C's chunk as a relay");
    // A also authors its OWN `tasks/a1` locally.
    a.apply_mutation_crdt(&insert("tasks", "a1", json!({"title": "from-a"}), 2), &idx)
        .unwrap();

    // A → B: capture the (origin_source, record_ids) the gate observes per chunk.
    let mut seen: Vec<(Option<String>, Vec<String>)> = Vec::new();
    sync_stores_authorized(&mut a, &idx, &mut b, &idx, |_relay, env, _audit| {
        seen.push((env.origin_source.clone(), env.record_ids.clone()));
        true
    })
    .unwrap();

    // The relay's `record.remote_import` row now PRESERVES the touched record ids
    // (`review 092 #2`: a forwarded chunk must still name a concrete record so the
    // receiver's envelope-metadata gate does not fail closed), so the forwarded and
    // local chunks can be located by their record ids directly.
    let c_chunk = seen
        .iter()
        .find(|(_, ids)| ids.iter().any(|r| r == "c1"))
        .expect("C's forwarded chunk carries its record id c1");
    let a_chunk = seen
        .iter()
        .find(|(_, ids)| ids.iter().any(|r| r == "a1"))
        .expect("A's own chunk carries its record id a1");

    // C's forwarded chunk is gated against C (the ORIGINAL author), not relay A.
    assert_eq!(
        c_chunk.0.as_deref(),
        Some("peer:33"),
        "the forwarded chunk is gated against C (the original author), not relay A"
    );
    // A's locally-authored chunk has no origin (relay == author).
    assert!(a_chunk.0.is_none(), "A's own write has no origin_source");

    // Exactly one chunk carries an origin_source — the single forwarded one.
    let forwarded = seen.iter().filter(|(o, _)| o.is_some()).count();
    assert_eq!(forwarded, 1, "exactly one forwarded chunk carries an origin_source: {seen:?}");
}

#[test]
fn original_author_survives_two_relay_hops() {
    // review 092 #1 (multi-hop): C authors a chunk; A imports it (hop 1); B imports it
    // FROM A (hop 2). After two relay hops the chunk is staged from B toward D and must
    // STILL be gated against C — not A, and not B. This proves each import preserves the
    // ORIGINAL author (and the record id) in its remote-import oplog row rather than
    // overwriting it with the immediate sender.
    let idx = IndexManager::new();
    let mut c = Store::open_in_memory().unwrap().with_crdt_peer_id(33); // author
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11); // relay 1
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22); // relay 2
    let mut d = Store::open_in_memory().unwrap().with_crdt_peer_id(44); // final receiver

    c.apply_mutation_crdt(&insert("tasks", "c1", json!({"title": "from-c"}), 1), &idx)
        .unwrap();
    assert_eq!(pull(&mut a, &c, &idx).unwrap(), 1, "hop 1: A imports C's chunk");
    assert_eq!(pull(&mut b, &a, &idx).unwrap(), 1, "hop 2: B imports it from relay A");

    // B -> D: the chunk B forwards must carry C's ORIGINAL source and its record id,
    // even though B got it from A (not C).
    let mut seen: Vec<(Option<String>, Vec<String>)> = Vec::new();
    sync_stores_authorized(&mut b, &idx, &mut d, &idx, |_relay, env, _audit| {
        seen.push((env.origin_source.clone(), env.record_ids.clone()));
        true
    })
    .unwrap();

    assert_eq!(seen.len(), 1, "exactly one chunk staged from B");
    assert_eq!(
        seen[0].0.as_deref(),
        Some("peer:33"),
        "after two relay hops the chunk is still gated against C (origin), not A or B"
    );
    assert!(
        seen[0].1.iter().any(|r| r == "c1"),
        "the record id c1 survives both relay hops: {seen:?}"
    );
}

#[test]
fn forwarded_chunk_with_unrecoverable_origin_is_staged_malformed() {
    // review 092 #1 (fail-closed twin): a relayed chunk whose `record.remote_import`
    // oplog row names NO recoverable original `source` must NOT be attributed to the
    // relay. It is staged `malformed` so the apply boundary denies it fail-closed —
    // a relay cannot launder a write whose author it cannot prove by having the
    // receiver fall back to the relay's own (trusted) identity.
    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut relay = Store::open_in_memory().unwrap().with_crdt_peer_id(33);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // An author writes a well-formed `collection/tasks` chunk (so the DOC ID is fine;
    // only the provenance is at issue).
    author
        .apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "from-author"}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("tasks");
    let staged: Vec<RemoteChunk> = author
        .get_chunks(&doc_id)
        .unwrap()
        .into_iter()
        .map(|c| RemoteChunk {
            doc_id: doc_id.clone(),
            chunk_id: exchanged_chunk_id(&c.format, &c.payload),
            format: c.format,
            payload: c.payload,
            author_actor_id: None,
            record_ids: Vec::new(),
            schema_version: None,
            registry_collection: None,
            delete_mutation_at: None,
        })
        .collect();
    // The relay imports the chunk with an EMPTY source string: its `record.remote_import`
    // oplog row records `"source": ""`, which `oplog_index` treats as UNRECOVERABLE
    // provenance (a relayed chunk whose author was lost), distinct from a local write.
    relay.apply_remote_chunks(&staged, "", &idx).unwrap();

    // Stage relay -> receiver. The chunk's origin row is a remote-import with an empty
    // source, so the envelope MUST be flagged malformed (fail closed). A deny-on-
    // malformed gate skips it; the receiver imports nothing.
    let mut seen: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    let report = sync_stores_authorized(&mut relay, &idx, &mut receiver, &idx, |_src, env, _audit| {
        seen.push((env.collection.clone(), env.origin_source.clone(), env.malformed.clone()));
        env.malformed.is_none()
    })
    .unwrap();

    assert_eq!(report.chunks_denied, 1, "the unrecoverable-origin chunk is denied");
    assert_eq!(seen.len(), 1, "exactly one chunk staged");
    assert!(seen[0].1.is_none(), "no original author was recovered: {seen:?}");
    assert!(
        seen[0]
            .2
            .as_deref()
            .map(|m| m.contains("no recoverable original author"))
            .unwrap_or(false),
        "the forwarded chunk with no usable source is flagged malformed: {seen:?}"
    );
    assert!(
        receiver.get_record("tasks", "t1").unwrap().is_none(),
        "the relay could not launder the sourceless chunk into the receiver"
    );
}

#[test]
fn non_collection_doc_id_is_staged_malformed_and_denied() {
    // review 092 #2: a chunk under a doc id that is NOT a `collection/<name>` records
    // doc must be staged with `malformed` set (and an empty collection) so the apply
    // boundary denies it fail-closed instead of guessing a collection from the raw
    // doc id. Here a deny-everything-malformed gate confirms the staged envelope.
    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    // Put a chunk under a non-records doc id directly (e.g. an applet src doc).
    a.put_chunk("applet/src", "chunk-0001", SYNC_CHUNK_FORMAT, b"opaque")
        .unwrap();

    let mut seen_malformed: Vec<(String, Option<String>)> = Vec::new();
    let report = sync_stores_authorized(&mut a, &idx, &mut b, &idx, |_src, env, _audit| {
        seen_malformed.push((env.collection.clone(), env.malformed.clone()));
        // The gate here always allows; the MECHANISM under test is that the
        // envelope is flagged malformed so the real core gate (which denies on
        // `malformed`) fails closed. The seam's allow does still import the opaque
        // chunk, but its non-collection doc never materializes a record.
        env.malformed.is_none()
    })
    .unwrap();

    assert_eq!(report.chunks_denied, 1, "the malformed-doc chunk was denied");
    assert_eq!(seen_malformed.len(), 1);
    assert_eq!(seen_malformed[0].0, "", "malformed envelope names no collection");
    assert!(
        seen_malformed[0].1.is_some(),
        "the non-collection doc id is flagged malformed: {seen_malformed:?}"
    );
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

/// DL-20 review 171 (P1): a synced DELETE must carry its `mutation_at` across the sync
/// boundary so the RECEIVER's monotone restore clock counts the imported delete.
///
/// The local delete path records the delete's logical timestamp on its oplog row as
/// `mutation_at`, and `record_history` recovers the tombstoned version's WHEN from it —
/// so the monotone default restore clock (`max(logical_at) + 1`) lands strictly after the
/// delete it undid (review 169). But that WHEN lived ONLY on the author's local oplog: the
/// remote-import path dropped `mutation_at`, so a peer importing `insert@1 -> patch@2 ->
/// delete@100` saw the tombstone with `logical_at = None`. An omitted `db.restore` there
/// would derive its default from `max(1, 2) + 1 = 3` — BEFORE the synced `delete@100` —
/// violating the monotone restore contract. This pins the fix: the delete's WHEN now rides
/// the staged chunk's metadata onto the receiver's `record.remote_import` oplog row.
///
/// ## Determinism (review 171 round 2)
///
/// The proof is split into two parts so it is STABLE across every build/scheduling state —
/// the round-1 form depended on the delete chunk surviving content-addressing as a single
/// distinct staged entry and on the change feed having exactly one entry per version, both
/// of which can vary with how Loro splits the incremental export and with `list_ops`
/// ordering, making the assertion flaky:
///
///   * Part A — the SEAM threading — is proven directly and deterministically by handing
///     `import_remote_chunk_tx` (via [`Store::apply_remote_chunks`]) a delete [`RemoteChunk`]
///     built EXACTLY as `missing_chunks_for_doc` builds it (`delete_mutation_at: Some(100)`),
///     with NO dependence on content-addressing or chunk splitting. The receiver's
///     `record.remote_import` row must carry `mutation_at = 100`. This is the load-bearing
///     "the WHEN crosses the boundary" assertion, made on the single code path the seam uses.
///
///   * Part B — the END-TO-END convergence + monotone clock — runs the real `sync_stores`
///     and asserts the RECEIVER's observable invariants with set/extremum checks that do not
///     depend on chunk count or feed-entry ordering: the SET of `mutation_at` values carried
///     by the receiver's remote-import rows is exactly `{100}` (no row carries a WRONG WHEN,
///     and the delete's WHEN is present), the change feed's tombstoned version reports
///     `Some(100)`, and the monotone default clock `max(logical_at) + 1` is `101 > 100`.
#[test]
fn synced_late_delete_carries_mutation_at_so_receiver_restore_clock_exceeds_it() {
    let idx = IndexManager::new();

    // ---- Part A: the seam threading, proven on the single import code path ---------------
    //
    // Hand the receiver a delete chunk built EXACTLY as the sync seam's `missing_chunks_for_doc`
    // builds it — `delete_mutation_at: Some(100)` — and assert `import_remote_chunk_tx` writes
    // that WHEN onto the receiver's `record.remote_import` oplog row. This exercises the fix
    // (`OplogPayload::remote_import` → `mutation_at`) with NO dependence on content-addressing,
    // chunk splitting, or feed ordering, so it cannot flake.
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    author
        .apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "draft"}), 1), &idx)
        .unwrap();
    author
        .apply_mutation_crdt(
            &Mutation::Delete {
                collection: "tasks".into(),
                id: "t1".into(),
                logical_at: Some(100),
            },
            &idx,
        )
        .unwrap();
    let doc_id = collection_doc_id("tasks");
    // Stage EVERY author chunk as the seam would, attaching the delete's WHEN to ALL of them
    // (the seam attaches it only to the delete chunk; over-attaching here would only make the
    // assertion STRICTER, but we keep it faithful: the delete's WHEN rides the delete chunk).
    // We mirror `missing_chunks_for_doc`'s shape directly so the test stays pinned to the seam
    // contract without reaching into its private staging.
    let delete_when = delete_when_for_chunks(&author, &doc_id);
    let staged: Vec<RemoteChunk> = author
        .get_chunks(&doc_id)
        .unwrap()
        .into_iter()
        .map(|c| {
            let exchanged = exchanged_chunk_id(&c.format, &c.payload);
            RemoteChunk {
                doc_id: doc_id.clone(),
                chunk_id: exchanged.clone(),
                format: c.format,
                payload: c.payload,
                author_actor_id: None,
                record_ids: vec!["t1".into()],
                schema_version: None,
                registry_collection: None,
                // The delete's WHEN rides ONLY the chunk the author's delete oplog row named.
                delete_mutation_at: delete_when.get(&c.chunk_id).copied(),
            }
        })
        .collect();
    let mut direct_receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    direct_receiver
        .apply_remote_chunks(&staged, "peer:11", &idx)
        .unwrap();
    // The SET of `mutation_at` values carried by the receiver's remote-import rows is exactly
    // {100}: the delete row carries the origin WHEN, no other row carries a (wrong) WHEN.
    assert_eq!(
        remote_import_mutation_ats(&direct_receiver),
        std::collections::BTreeSet::from([100]),
        "import_remote_chunk_tx must write the delete's mutation_at=100 onto the \
         receiver's record.remote_import row (the seam threading)"
    );

    // ---- Part B: end-to-end convergence + monotone clock via the real sync seam ----------
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // insert@1 -> patch@2 -> delete@100 (a LATE delete: WHEN well past the data frontier).
    author
        .apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "draft"}), 1), &idx)
        .unwrap();
    author
        .apply_mutation_crdt(
            &Mutation::Patch {
                collection: "tasks".into(),
                id: "t1".into(),
                fields: json!({"title": "final"}).as_object().unwrap().clone(),
                logical_at: Some(2),
            },
            &idx,
        )
        .unwrap();
    author
        .apply_mutation_crdt(
            &Mutation::Delete {
                collection: "tasks".into(),
                id: "t1".into(),
                logical_at: Some(100),
            },
            &idx,
        )
        .unwrap();

    // Sync the whole history to the fresh receiver (the M0b always-allow seam).
    let report = sync_stores(&mut author, &idx, &mut receiver, &idx).unwrap();
    assert!(report.total_chunks_moved() > 0, "the history must move");
    // The record is a tombstone on both peers after convergence.
    assert!(receiver.get_record("tasks", "t1").unwrap().is_none());

    // (1) The imported delete's `record.remote_import` oplog row physically carries the origin
    // delete's logical timestamp as `mutation_at` — the metadata crossed the sync boundary
    // (the fix). Asserted as a SET so the proof is invariant to how many chunks the import
    // produced and to `list_ops` ordering: the set of WHENs the remote-import rows carry is
    // exactly {100} — the delete's WHEN is present and no row carries a wrong WHEN.
    assert_eq!(
        remote_import_mutation_ats(&receiver),
        std::collections::BTreeSet::from([100]),
        "the synced delete must carry mutation_at=100 onto the receiver's remote-import row"
    );

    // (2) On the receiver, the change feed surfaces the imported delete's WHEN. The delete's
    // WHEN (=100) is the MAXIMUM logical timestamp in the feed (the late delete jumps past the
    // live-state frontier), and the entry carrying it is a tombstone (deleted) version. This is
    // asserted by the WHEN value, NOT by feed position / `version`: a sync-receiver stores
    // content-addressed chunk ids, so every entry's `version` (the `chunk-NNNN` frontier)
    // degrades to 0 — feed ORDER among them is not meaningful, so the proof keys on the
    // logical WHEN instead. Before the fix the delete's WHEN was dropped, the feed's max WHEN
    // was 2, and NO entry reported 100.
    let feed = receiver.record_history("tasks", "t1").unwrap();
    let max_when = feed.iter().filter_map(|e| e.logical_at).max();
    assert_eq!(
        max_when,
        Some(100),
        "the feed's maximum WHEN must be the synced late delete's (=100), not None/2"
    );
    assert!(
        feed.iter()
            .filter(|e| e.logical_at == Some(100))
            .all(|e| e.state.is_none()),
        "every entry carrying the delete WHEN (=100) is the tombstoned (deleted) version"
    );

    // (3) The default restore clock B derives the SAME way core's `monotone_restore_clock`
    // does — `max(history.logical_at) + 1` over the receiver's change feed — now COUNTS the
    // imported delete in its frontier, so it is 101: strictly greater than the synced
    // `delete@100`. Restoring on B with this omitted-clock default therefore stamps a
    // version AFTER the delete it reverses, honoring the monotone contract on the receiving
    // peer. BEFORE the fix the imported delete's WHEN was `None`, the frontier was
    // `max(1, 2) = 2`, and the default was `3` — BEFORE the very delete the restore undoes,
    // the non-monotone bug. (Mirror core's cast: the frontier is a u64 logical clock and the
    // restore takes an i64, so `+1` is computed on the cast; the clock never nears i64::MAX.)
    let monotone_default = feed.iter().filter_map(|e| e.logical_at).max().unwrap() as i64 + 1;
    assert_eq!(
        monotone_default, 101,
        "receiver's monotone restore default must count the synced delete@100 (=101), not 3"
    );
    assert!(
        monotone_default > 100,
        "the default restore WHEN must be strictly after the synced delete@100"
    );
}

/// The set of `mutation_at` values carried by a store's `record.remote_import` oplog rows.
/// Used by the review-171 regression as an ORDER- and COUNT-independent witness that the
/// synced delete's WHEN crossed the sync boundary onto the receiver's remote-import row(s):
/// the set is `{100}` exactly when the delete's WHEN is carried and no other imported row
/// carries a (wrong) WHEN, regardless of how the import split into chunks or how `list_ops`
/// orders them.
fn remote_import_mutation_ats(store: &Store) -> std::collections::BTreeSet<i64> {
    store
        .list_ops()
        .unwrap()
        .into_iter()
        .filter(|o| o.kind == "record.remote_import")
        .filter_map(|o| {
            serde_json::from_slice::<Value>(&o.payload)
                .ok()
                .and_then(|v| v.get("mutation_at").and_then(Value::as_i64))
        })
        .collect()
}

/// Map each of `doc_id`'s local chunk ids to the delete `mutation_at` its oplog row recorded
/// (DL-20 review 169) — i.e. exactly the WHEN the sync seam's `missing_chunks_for_doc` would
/// attach to that chunk's staged [`RemoteChunk`]. A non-delete chunk's id is absent (its row
/// carries no `mutation_at`). Lets the deterministic seam-threading assertion stage chunks
/// the SAME way the seam does without reaching into its private staging path.
fn delete_when_for_chunks(
    store: &Store,
    doc_id: &str,
) -> std::collections::BTreeMap<String, i64> {
    let mut out = std::collections::BTreeMap::new();
    for op in store.list_ops().unwrap() {
        let Some(local_id) = op.op_id.strip_prefix(&format!("{doc_id}#")) else {
            continue;
        };
        if let Some(at) = serde_json::from_slice::<Value>(&op.payload)
            .ok()
            .and_then(|v| v.get("mutation_at").and_then(Value::as_i64))
        {
            out.insert(local_id.to_string(), at);
        }
    }
    out
}

/// Review 139 (P1) — DL-13 migration chunks must SYNC to peers and advance the
/// receiver's `schema_version`.
///
/// Setup mirrors the existing two-store `sync_stores` tests: peer A seeds two
/// `expenses` records (`amount` as int) through the DL-4 CRDT path, then applies a
/// `widen_int_to_float` migration (`Store::apply_migration`) that rewrites the CRDT
/// source of truth and bumps A to `schema_version 2`. Peer B is a fresh peer at the
/// initial version. After `sync_stores(A, B)`:
///   - B holds the MIGRATED (float) record values after its rebuild — the migration
///     chunk reached B (before review 139 it had no per-chunk oplog row, fell back to
///     a generic write with empty record_ids, and was DROPPED at the apply gate);
///   - B's `schema_version == to_schema_version` (2) — the chunk carried the version
///     it advances, and the receiver bumped to it IN the same import txn (no drift);
///   - a second sync is a pure no-op (the migration chunk converged, idempotent).
#[test]
fn migration_chunk_syncs_to_peer_and_advances_receiver_schema_version() {
    use forge_schema::{FieldTransform, FieldType, MigrationDescriptor};

    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // A seeds two int `amount` records through the real CRDT write path, so the
    // values live in the source of truth the migration rewrites.
    a.apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10}), 1), &idx)
        .unwrap();
    a.apply_mutation_crdt(&insert("expenses", "e2", json!({"amount": 20}), 2), &idx)
        .unwrap();

    // A widens `amount` int → float; A advances to schema_version 2 and rewrites the
    // CRDT source of truth (the migration chunk + its per-chunk oplog row).
    let widen = MigrationDescriptor {
        collection: "expenses".into(),
        from_schema_version: 1,
        to_schema_version: 2,
        transforms: vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            name: "amount".into(),
            to: FieldType::FloatNum,
        }],
    };
    let outcome = a.apply_migration(&widen, &idx).unwrap();
    assert!(outcome.applied);
    assert_eq!(a.schema_version().unwrap(), 2);
    // B is still at the initial version before any sync.
    assert_eq!(b.schema_version().unwrap(), 1);

    // Converge A → B (and back). The migration chunk now carries a per-chunk oplog row
    // (discoverable by the chunk→metadata join) with the migrated record ids + the
    // schema-version pair, so it is staged, NOT denied, and imported.
    let report = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert!(report.total_chunks_moved() > 0, "the migration chunk must move A → B");

    // B has the MIGRATED (float) values after its rebuild — the migration reached it.
    for id in ["e1", "e2"] {
        let env = b.get_record("expenses", id).unwrap().unwrap();
        assert!(
            env.fields["amount"].is_f64(),
            "B/{id} must hold the migrated float value after sync, got {:?}",
            env.fields["amount"]
        );
    }
    assert_eq!(b.get_record("expenses", "e1").unwrap().unwrap().fields["amount"], json!(10.0));
    assert_eq!(b.get_record("expenses", "e2").unwrap().unwrap().fields["amount"], json!(20.0));

    // B advanced to the migration's target version IN the same import txn — no peer
    // can materialize migrated values while staying behind at the old version.
    assert_eq!(
        b.schema_version().unwrap(),
        2,
        "B's schema_version must advance to the migration target on import (no drift)"
    );

    // The migration chunk durably survives B's own DL-6 rebuild (it landed in B's
    // crdt_chunks, not just its projection), and B's version stays advanced.
    b.rebuild_projection(&idx).unwrap();
    assert_eq!(b.get_record("expenses", "e1").unwrap().unwrap().fields["amount"], json!(10.0));
    assert_eq!(b.schema_version().unwrap(), 2, "B stays at v2 across a CRDT rebuild");

    // A second sync is a pure no-op (the migration chunk converged) and B's version
    // is unchanged — the receiver advance is monotone + idempotent.
    let again = sync_stores(&mut a, &idx, &mut b, &idx).unwrap();
    assert_eq!(again.total_chunks_moved(), 0, "converged: the migration chunk does not re-move");
    assert_eq!(b.schema_version().unwrap(), 2, "B's version is unchanged on a converged re-sync");
}

/// Review 145 (P1) — a migration's schema-affecting metadata must survive a RELAY hop at
/// the storage/sync seam (the mechanism the core's three-peer test exercises end to end).
///
/// A authors a `widen` migration; B imports it FROM A (hop 1), recording a
/// `record.remote_import` oplog row. When B relays to C (hop 2), that row must still carry
/// the migration's `to` schema_version so the staged envelope keeps `schema_version =
/// Some(2)` — i.e. the seam re-stages the chunk as a SCHEMA-AFFECTING op (so the authorizer
/// re-gates schema_write at the second hop) and C advances its `schema_version` to the
/// target. Before the fix B's relay row dropped `to`, the envelope went `None`, and C
/// imported the migrated data as a plain record write that left its version at 1.
#[test]
fn migration_metadata_survives_a_relay_hop_to_the_third_peer() {
    use forge_schema::{FieldTransform, FieldType, MigrationDescriptor};

    let idx = IndexManager::new();
    let mut a = Store::open_in_memory().unwrap().with_crdt_peer_id(11); // author
    let mut b = Store::open_in_memory().unwrap().with_crdt_peer_id(22); // relay
    let mut c = Store::open_in_memory().unwrap().with_crdt_peer_id(33); // final receiver

    a.apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10}), 1), &idx)
        .unwrap();
    a.apply_mutation_crdt(&insert("expenses", "e2", json!({"amount": 20}), 2), &idx)
        .unwrap();
    let widen = MigrationDescriptor {
        collection: "expenses".into(),
        from_schema_version: 1,
        to_schema_version: 2,
        transforms: vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            name: "amount".into(),
            to: FieldType::FloatNum,
        }],
    };
    a.apply_migration(&widen, &idx).unwrap();
    assert_eq!(a.schema_version().unwrap(), 2);

    // Hop 1: B imports A's migration FROM A (B is now only a RELAY for it — its oplog row
    // for the migration chunk is a `record.remote_import`, not a `schema.migration`).
    assert!(pull(&mut b, &a, &idx).unwrap() > 0, "B imports A's chunks");
    assert_eq!(b.schema_version().unwrap(), 2, "B advanced on the first hop");

    // The crux: stage B -> C and capture the envelope the seam recovers from B's RELAY row.
    // The migration's target version must be recovered from the `record.remote_import` row
    // (review 145), so the staged op is schema-affecting at the SECOND hop too.
    let mut saw_migration_envelope = false;
    sync_stores_authorized(&mut b, &idx, &mut c, &idx, |_src, env, _audit| {
        if env.collection == "expenses" && env.schema_version == Some(2) {
            saw_migration_envelope = true;
        }
        true
    })
    .unwrap();
    assert!(
        saw_migration_envelope,
        "the relayed migration chunk must still carry schema_version=Some(2) (metadata survived B's relay row)"
    );

    // C holds the MIGRATED (float) values AND advanced its schema_version through the relay.
    assert_eq!(c.get_record("expenses", "e1").unwrap().unwrap().fields["amount"], json!(10.0));
    assert_eq!(c.get_record("expenses", "e2").unwrap().unwrap().fields["amount"], json!(20.0));
    assert_eq!(
        c.schema_version().unwrap(),
        2,
        "C advanced to the migration target across TWO hops — not left behind as a plain write"
    );

    // The migration is durable in C's own CRDT source of truth: a DL-6 rebuild reproduces
    // the migrated values and an index reconstructed afterward serves them.
    c.rebuild_projection(&idx).unwrap();
    assert_eq!(c.get_record("expenses", "e1").unwrap().unwrap().fields["amount"], json!(10.0));
    assert_eq!(c.schema_version().unwrap(), 2, "C stays at v2 across its own rebuild");
    let mut c_idx = IndexManager::new();
    c.create_index(&mut c_idx, "expenses", "f_amount", CreateIndexKind::Value)
        .expect("C reconstructs an index over the migrated f_amount");
    assert_eq!(
        c.get_record("expenses", "e1").unwrap().unwrap().field_ids["f_amount"].as_f64(),
        Some(10.0)
    );
}

/// Review-w9 P1 (DL-13) — the per-collection registry merge on a migration import must NOT
/// be gated on the workspace-GLOBAL `schema_version` actually advancing. `schema_version` is
/// ONE workspace-wide counter, but registry evolution is PER-COLLECTION: a receiver whose
/// global version already equals the migration's target — reached via UNRELATED schema work on
/// OTHER collections — must STILL merge the carried `registry_collection`, or it imports the
/// migrated records while leaving its registry behind (data ahead of schema, the drift class
/// review 143 closed). The old code merged the registry only `if advanced`, so on a store
/// already at the target `advance_schema_version_if_newer` returned false and the merge was
/// SKIPPED. This drives `apply_remote_chunks` directly into a store pre-set to the target.
#[test]
fn migration_chunk_merges_registry_even_when_receiver_already_at_target_version() {
    use forge_domain::ActorId;
    use forge_schema::{FieldType, SchemaChange, SchemaRegistry};
    use forge_storage::SCHEMA_REGISTRY_KEY;

    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // The author writes one `expenses` chunk (the migrated record payload) through the real
    // CRDT path; its bytes become the migration chunk the receiver imports.
    author
        .apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10.0}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("expenses");

    // The migration's target version. The RECEIVER is pre-set to this SAME version via a
    // monotone advance — modelling a peer whose global counter already reached the target via
    // UNRELATED schema work — so `advance_schema_version_if_newer` returns advanced=false at
    // import (the precise trap that gated the registry merge in the buggy code).
    const TARGET: u64 = 2;
    receiver.advance_schema_version(TARGET).unwrap();
    assert_eq!(receiver.schema_version().unwrap(), TARGET, "receiver pre-set to the target version");

    // Build the EVOLVED `expenses` collection entry the migration carries: an `amount` float
    // field. This is a genuine `forge_schema::CollectionDef`, serialized as the carried
    // `registry_collection` payload `{ "name", "collection" }` the import merges.
    let mut authored_registry = SchemaRegistry::new();
    authored_registry
        .apply_change(SchemaChange::AddCollection { name: "expenses".into() })
        .unwrap();
    authored_registry
        .apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "amount".into(),
            ty: FieldType::FloatNum,
            indexed: false,
            required: false,
        })
        .unwrap();
    let expenses_def = authored_registry.collection("expenses").unwrap().clone();
    let carried_registry = json!({
        "name": "expenses",
        "collection": serde_json::to_value(&expenses_def).unwrap(),
    });

    // PRECONDITION: the receiver does NOT yet know `expenses` — its persisted registry is empty.
    assert!(
        receiver.kv_get("__forge/meta", SCHEMA_REGISTRY_KEY).unwrap().is_none(),
        "the receiver has no persisted registry before the import"
    );

    // Stage the author's chunk as a MIGRATION chunk carrying BOTH the target version AND the
    // evolved registry entry, then import it. Because the receiver is already at TARGET, the
    // version advance is a no-op — but the registry merge must STILL run.
    let staged: Vec<RemoteChunk> = author
        .get_chunks(&doc_id)
        .unwrap()
        .into_iter()
        .map(|c| RemoteChunk {
            doc_id: doc_id.clone(),
            chunk_id: exchanged_chunk_id(&c.format, &c.payload),
            format: c.format,
            payload: c.payload,
            author_actor_id: Some("peer:11".into()),
            record_ids: vec!["e1".into()],
            schema_version: Some(TARGET),
            registry_collection: Some(carried_registry.clone()),
            delete_mutation_at: None,
        })
        .collect();
    let imported = receiver.apply_remote_chunks(&staged, "peer:11", &idx).unwrap();
    assert_eq!(imported, 1, "the migration chunk is imported");

    // (records) The migrated record landed.
    assert_eq!(
        receiver.get_record("expenses", "e1").unwrap().unwrap().fields["amount"],
        json!(10.0)
    );
    // (version) Unchanged — the receiver was already at the target (this is exactly why the
    // buggy gate skipped the merge).
    assert_eq!(
        receiver.schema_version().unwrap(),
        TARGET,
        "the receiver's version stays at the target (no advance — the gate trap)"
    );

    // (registry) The CRUX: the carried `expenses` entry was MERGED into the receiver's persisted
    // registry EVEN THOUGH the global version did not advance.
    let persisted = receiver
        .kv_get("__forge/meta", SCHEMA_REGISTRY_KEY)
        .unwrap()
        .expect("the receiver persisted a registry after merging the carried collection");
    let merged: SchemaRegistry = serde_json::from_slice(&persisted).unwrap();
    let merged_col = merged
        .collection("expenses")
        .expect("the receiver merged the carried `expenses` collection despite no global advance");
    assert_eq!(merged_col, &expenses_def, "the merged entry equals the carried evolved collection");

    // A re-import of the SAME chunk is an idempotent no-op: no new chunk, the registry unchanged.
    let again = receiver.apply_remote_chunks(&staged, "peer:11", &idx).unwrap();
    assert_eq!(again, 0, "the converged re-import moves no chunk");
    let persisted_again = receiver
        .kv_get("__forge/meta", SCHEMA_REGISTRY_KEY)
        .unwrap()
        .unwrap();
    assert_eq!(persisted_again, persisted, "the re-import left the merged registry byte-identical");
}

/// Build the `{ "name", "collection" }` carried-registry payload a migration chunk
/// ships, from an evolved `expenses` registry's collection entry.
#[cfg(test)]
fn carried_expenses(registry: &forge_schema::SchemaRegistry) -> Value {
    let def = registry.collection("expenses").unwrap().clone();
    json!({ "name": "expenses", "collection": serde_json::to_value(&def).unwrap() })
}

/// Stage one chunk as a DL-13 migration chunk carrying `schema_version` + a
/// `registry_collection` payload, mirroring the sync seam's `RemoteChunk` shape.
#[cfg(test)]
fn migration_chunk(
    doc_id: &str,
    chunk: &forge_storage::ChunkRow,
    to_version: u64,
    carried_registry: Value,
) -> RemoteChunk {
    RemoteChunk {
        doc_id: doc_id.to_string(),
        chunk_id: exchanged_chunk_id(&chunk.format, &chunk.payload),
        format: chunk.format.clone(),
        payload: chunk.payload.clone(),
        author_actor_id: Some("peer:11".into()),
        record_ids: vec!["e1".into()],
        schema_version: Some(to_version),
        registry_collection: Some(carried_registry),
        delete_mutation_at: None,
    }
}

/// Review 147 (P1) — the per-collection registry merge on a migration import must be
/// FORWARD-ONLY, not a blind replace. A receiver whose `expenses` collection is
/// ALREADY evolved (here `amount` widened int → float, at the migration target version)
/// imports a DELAYED, OLDER migration chunk that carries `expenses` at the int_num
/// stage. Under the review-w9 always-merge fix this chunk reaches
/// `evolve_registry_collection_tx` (the receiver is already at the target, so the
/// version advance is a no-op but the registry merge still runs) — and a blind replace
/// would roll the registry BACKWARD to int_num while the global `schema_version` stays
/// newer. The forward-only guard must SKIP the stale carried def, leaving `amount` at
/// float_num, the records untouched, and the version unregressed.
#[test]
fn out_of_order_older_migration_chunk_does_not_roll_back_evolved_registry() {
    use forge_domain::ActorId;
    use forge_schema::{FieldType, SchemaChange, SchemaRegistry};
    use forge_storage::SCHEMA_REGISTRY_KEY;

    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // The author writes one `expenses` chunk (a record payload); its bytes are reused
    // as the body of the older (int-stage) migration chunk the receiver will import.
    author
        .apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 7}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("expenses");
    let chunk = author.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

    // Build the OLDER (int_num) `expenses` registry — the stage the delayed chunk carries.
    let mut int_registry = SchemaRegistry::new();
    int_registry
        .apply_change(SchemaChange::AddCollection { name: "expenses".into() })
        .unwrap();
    int_registry
        .apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: false,
            required: false,
        })
        .unwrap();

    // The receiver is ALREADY AHEAD: the SAME `f_alice_0` field widened int → float, at
    // the migration target version. Pre-seed that evolved registry + version directly.
    let mut float_registry = int_registry.clone();
    float_registry
        .apply_change(SchemaChange::WidenField {
            collection: "expenses".into(),
            field_id: "f_alice_0".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
    let evolved_def = float_registry.collection("expenses").unwrap().clone();
    receiver
        .kv_set(
            "__forge/meta",
            SCHEMA_REGISTRY_KEY,
            &serde_json::to_vec(&float_registry).unwrap(),
            "application/json",
        )
        .unwrap();
    const TARGET: u64 = 2;
    receiver.advance_schema_version(TARGET).unwrap();

    // The DELAYED OLDER chunk carries `expenses` at the int_num stage and the SAME
    // target version (so the version advance is a no-op and the registry merge runs).
    let stale = migration_chunk(&doc_id, &chunk, TARGET, carried_expenses(&int_registry));
    let imported = receiver.apply_remote_chunks(&[stale], "peer:11", &idx).unwrap();
    assert_eq!(imported, 1, "the (older) migration chunk is still imported");

    // (registry) The CRUX: `amount` stays FLOAT — the stale int carried def was skipped,
    // not blindly replaced, so the evolved registry is NOT rolled back.
    let persisted = receiver
        .kv_get("__forge/meta", SCHEMA_REGISTRY_KEY)
        .unwrap()
        .expect("the receiver still has its evolved registry");
    let merged: SchemaRegistry = serde_json::from_slice(&persisted).unwrap();
    let merged_col = merged.collection("expenses").unwrap();
    assert_eq!(
        merged_col, &evolved_def,
        "the receiver keeps its float `amount` — the older int chunk did not roll it back"
    );
    assert_eq!(
        *merged_col.field("f_alice_0").unwrap().ty(),
        FieldType::FloatNum,
        "amount must remain float_num (not regressed to int_num)"
    );

    // (records) The record landed unaffected by the registry decision.
    assert_eq!(
        receiver.get_record("expenses", "e1").unwrap().unwrap().fields["amount"],
        json!(7)
    );
    // (version) The global schema_version is not regressed.
    assert_eq!(
        receiver.schema_version().unwrap(),
        TARGET,
        "the workspace-global schema_version is not rolled back by a stale chunk"
    );
}

/// Review 147 (companion) — a NORMAL FORWARD migration chunk still applies its carried
/// registry. A receiver that knows `expenses` at the int_num stage imports a migration
/// chunk carrying the evolved float_num `expenses`; the forward-compatible superset must
/// be MERGED (`amount` becomes float), confirming the forward-only guard does not block
/// legitimate forward evolution.
#[test]
fn normal_forward_migration_chunk_still_applies_evolved_registry() {
    use forge_domain::ActorId;
    use forge_schema::{FieldType, SchemaChange, SchemaRegistry};
    use forge_storage::SCHEMA_REGISTRY_KEY;

    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    author
        .apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 9}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("expenses");
    let chunk = author.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

    // Receiver is at the int_num stage (knows `expenses.amount: int`, version 1).
    let mut int_registry = SchemaRegistry::new();
    int_registry
        .apply_change(SchemaChange::AddCollection { name: "expenses".into() })
        .unwrap();
    int_registry
        .apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "amount".into(),
            ty: FieldType::IntNum,
            indexed: false,
            required: false,
        })
        .unwrap();
    receiver
        .kv_set(
            "__forge/meta",
            SCHEMA_REGISTRY_KEY,
            &serde_json::to_vec(&int_registry).unwrap(),
            "application/json",
        )
        .unwrap();

    // The migration chunk carries the EVOLVED float `expenses` (a forward superset).
    let mut float_registry = int_registry.clone();
    float_registry
        .apply_change(SchemaChange::WidenField {
            collection: "expenses".into(),
            field_id: "f_alice_0".into(),
            to: FieldType::FloatNum,
        })
        .unwrap();
    let evolved_def = float_registry.collection("expenses").unwrap().clone();

    let fwd = migration_chunk(&doc_id, &chunk, 2, carried_expenses(&float_registry));
    receiver.apply_remote_chunks(&[fwd], "peer:11", &idx).unwrap();

    // The forward-compatible carried def was MERGED — `amount` is now float_num.
    let persisted = receiver.kv_get("__forge/meta", SCHEMA_REGISTRY_KEY).unwrap().unwrap();
    let merged: SchemaRegistry = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(
        merged.collection("expenses").unwrap(),
        &evolved_def,
        "a forward migration chunk must apply its evolved registry entry"
    );
    assert_eq!(receiver.schema_version().unwrap(), 2, "the receiver advanced to the target");
}

/// Review 159 (P1) — CONCURRENT ADDITIVE divergence must converge to the field UNION,
/// not be skipped. The receiver already knows `expenses` with its OWN actor-scoped
/// field `f_bob_0` (added offline from the shared base). It imports an authorized
/// migration carrying a DIFFERENT field `f_alice_0` evolved from the SAME base — a
/// carried def that does NOT contain `f_bob_0`. The retired forward-only-superset gate
/// would reject this carried entry as "not a superset" and SKIP it, leaving the
/// migrated `f_alice_0` data with no registry field. The DL-11 union merge must instead
/// merge to a registry holding BOTH `f_bob_0` AND `f_alice_0`, with the migrated record
/// landing.
#[test]
fn concurrent_additive_migration_chunk_converges_registry_to_the_field_union() {
    use forge_domain::ActorId;
    use forge_schema::{FieldType, SchemaChange, SchemaRegistry};
    use forge_storage::SCHEMA_REGISTRY_KEY;

    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(22);

    // The author writes one `expenses` chunk (carries alice's new field value); its
    // bytes become the migration chunk body the receiver imports.
    author
        .apply_mutation_crdt(&insert("expenses", "e1", json!({"note": "hi"}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("expenses");
    let chunk = author.get_chunks(&doc_id).unwrap().into_iter().next().unwrap();

    // The shared base: `expenses` with no fields (both peers branch from here).
    let mut base = SchemaRegistry::new();
    base.apply_change(SchemaChange::AddCollection { name: "expenses".into() }).unwrap();

    // BOB's branch (the receiver's LOCAL state): added field `f_bob_0` (`flag: bool`).
    let mut bob_registry = base.clone();
    bob_registry
        .apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("bob"),
            name: "flag".into(),
            ty: FieldType::Bool,
            indexed: false,
            required: false,
        })
        .unwrap();
    receiver
        .kv_set(
            "__forge/meta",
            SCHEMA_REGISTRY_KEY,
            &serde_json::to_vec(&bob_registry).unwrap(),
            "application/json",
        )
        .unwrap();

    // ALICE's branch (the carried migration): added a DIFFERENT field `f_alice_0`
    // (`note: text`) from the SAME base. The carried def lacks bob's field.
    let mut alice_registry = base.clone();
    alice_registry
        .apply_change(SchemaChange::AddField {
            collection: "expenses".into(),
            actor: ActorId::new("alice"),
            name: "note".into(),
            ty: FieldType::Text,
            indexed: false,
            required: false,
        })
        .unwrap();

    // Import alice's migration chunk carrying the alice-only `expenses` entry.
    let chunk_msg = migration_chunk(&doc_id, &chunk, 2, carried_expenses(&alice_registry));
    let imported = receiver.apply_remote_chunks(&[chunk_msg], "peer:11", &idx).unwrap();
    assert_eq!(imported, 1, "the concurrent-additive migration chunk is imported");

    // (registry) The CRUX: the merged `expenses` holds BOTH fields — bob's local field
    // is NOT dropped (the carried def lacked it) and alice's carried field is added.
    let persisted = receiver
        .kv_get("__forge/meta", SCHEMA_REGISTRY_KEY)
        .unwrap()
        .expect("the receiver persisted a merged registry");
    let merged: SchemaRegistry = serde_json::from_slice(&persisted).unwrap();
    let merged_col = merged.collection("expenses").unwrap();
    assert!(
        merged_col.field("f_bob_0").is_some(),
        "bob's local field must survive — the carried def lacking it must NOT drop it (159)"
    );
    assert!(
        merged_col.field("f_alice_0").is_some(),
        "alice's carried field must be added — concurrent-additive must converge to the union (159)"
    );
    // The merge is a forward evolution of the receiver's prior LOCAL state — no actor
    // counter regressed, no field lost (it strictly added alice's field).
    assert!(merged.validate_compatibility(&bob_registry).is_ok());

    // (records) The migrated record landed with alice's `note`.
    assert_eq!(
        receiver.get_record("expenses", "e1").unwrap().unwrap().fields["note"],
        json!("hi")
    );
}

/// Review 145 (fail-closed): a `record.remote_import` oplog row MARKED schema-affecting
/// (`is_migration: true`) but whose `to` target is UNRECOVERABLE must be staged `malformed`
/// so a relaying peer DENIES it rather than importing migrated data as a plain record write
/// that silently skips the schema advance. We forge such a row by importing a chunk whose
/// `RemoteChunk` carries `schema_version` but on a store, then corrupt nothing — instead we
/// directly assert the seam's fail-closed path via a hand-built remote-import row missing a
/// usable `to`.
#[test]
fn relay_row_marked_migration_without_recoverable_target_is_staged_malformed() {
    let idx = IndexManager::new();
    let mut author = Store::open_in_memory().unwrap().with_crdt_peer_id(11);
    let mut relay = Store::open_in_memory().unwrap().with_crdt_peer_id(22);
    let mut receiver = Store::open_in_memory().unwrap().with_crdt_peer_id(33);

    // The author writes a well-formed `expenses` chunk.
    author
        .apply_mutation_crdt(&insert("expenses", "e1", json!({"amount": 10}), 1), &idx)
        .unwrap();
    let doc_id = collection_doc_id("expenses");
    // Stage the chunk into the relay as a MIGRATION chunk but with an UNRECOVERABLE target:
    // `schema_version: Some(0)`. The relay's remote-import row records `is_migration: true`
    // (the marker) but a `to` of 0, which `oplog_index` treats as unrecoverable metadata.
    let staged: Vec<RemoteChunk> = author
        .get_chunks(&doc_id)
        .unwrap()
        .into_iter()
        .map(|c| RemoteChunk {
            doc_id: doc_id.clone(),
            chunk_id: exchanged_chunk_id(&c.format, &c.payload),
            format: c.format,
            payload: c.payload,
            author_actor_id: Some("peer:11".into()),
            record_ids: vec!["e1".into()],
            schema_version: Some(0),
            registry_collection: None,
            delete_mutation_at: None,
        })
        .collect();
    relay.apply_remote_chunks(&staged, "peer:11", &idx).unwrap();

    // Stage relay -> receiver. The relay row is marked schema-affecting but its `to` is
    // unrecoverable, so the envelope MUST be flagged malformed (fail closed). A deny-on-
    // malformed gate skips it; the receiver imports nothing.
    let mut seen: Vec<Option<String>> = Vec::new();
    let report = sync_stores_authorized(&mut relay, &idx, &mut receiver, &idx, |_src, env, _audit| {
        seen.push(env.malformed.clone());
        env.malformed.is_none()
    })
    .unwrap();

    assert_eq!(report.chunks_denied, 1, "the unrecoverable-metadata migration row is denied");
    assert!(
        seen.iter().any(|m| m
            .as_deref()
            .map(|m| m.contains("unrecoverable migration metadata"))
            .unwrap_or(false)),
        "the schema-affecting row with no usable target is flagged malformed: {seen:?}"
    );
    assert!(
        receiver.get_record("expenses", "e1").unwrap().is_none(),
        "the receiver imported nothing — a schema-affecting chunk with lost metadata is not laundered as a plain write"
    );
}
