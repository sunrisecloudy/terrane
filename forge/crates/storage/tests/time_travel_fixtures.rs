//! Data-driven DL-20 time-travel harness over `forge/fixtures/time-travel/`.
//!
//! Each fixture seeds a record through the real DL-4 CRDT mutation path
//! (`apply_mutation_crdt`), then asserts one of five behaviors by its manifest
//! `kind`:
//!
//! - `history`: `Store::record_history` equals the listed who/when/what + state
//!   entries, in version order.
//! - `state_at`: `Store::record_state_at` reconstructs the listed fields (or
//!   absence) at each past version.
//! - `restore`: `Store::restore_record` appends a NEW version; the current state,
//!   the prior-intact reconstructions, and the history growth match (the
//!   NON-DESTRUCTIVE contract).
//! - `deterministic`: history reads are byte-stable and a restore replays
//!   deterministically (no wall clock in the replayable path).
//! - `retention`: `compact_history` with a `RetentionPolicy` keeps the
//!   within-window change-feed oplog rows and prunes beyond.
//!
//! A `ran == manifest.count` guard makes a silently-skipped case fail, and every
//! assertion reads back the real stored substrate (no faking).

use forge_storage::{
    collection_doc_id, CompactionOptions, IndexManager, Mutation, RetentionPolicy, Store,
};
use serde_json::Value;

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/time-travel")
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

/// Apply one seed mutation through the real DL-4 CRDT write path.
fn apply_seed(store: &mut Store, idx: &IndexManager, collection: &str, m: &Value) {
    let op = m["op"].as_str().expect("seed op");
    let id = m["id"].as_str().expect("seed id").to_string();
    let logical_at = m["logical_at"].as_i64();
    let mutation = match op {
        "insert" => Mutation::Insert {
            collection: collection.into(),
            id: Some(id),
            fields: obj(&m["fields"]),
            logical_at,
        },
        "update" => Mutation::Update {
            collection: collection.into(),
            id,
            fields: obj(&m["fields"]),
            logical_at,
        },
        "patch" => Mutation::Patch {
            collection: collection.into(),
            id,
            fields: obj(&m["fields"]),
            logical_at,
        },
        "delete" => Mutation::Delete {
            collection: collection.into(),
            id,
            logical_at,
        },
        other => panic!("unknown seed op {other}"),
    };
    store.apply_mutation_crdt(&mutation, idx).expect("seed mutation");
}

/// Seed the record, returning the store + index manager primed for assertions.
fn seed(fx: &Value) -> (Store, IndexManager, String, String) {
    let collection = fx["collection"].as_str().expect("collection").to_string();
    let record = fx["record"].as_str().expect("record").to_string();
    let mut store = Store::open_in_memory().unwrap();
    let idx = IndexManager::new();
    for m in fx["seed"].as_array().expect("seed array") {
        apply_seed(&mut store, &idx, &collection, m);
    }
    (store, idx, collection, record)
}

/// Compare a reconstructed envelope's display `fields` to the fixture's expected
/// `state_fields` object.
fn assert_state_fields(case: &str, env: Option<&forge_domain::RecordEnvelope>, expected: &Value) {
    let env = env.unwrap_or_else(|| panic!("{case}: expected a record state, got absent"));
    for (k, v) in expected.as_object().expect("state_fields object") {
        assert_eq!(
            env.fields.get(k),
            Some(v),
            "{case}: field {k} mismatch (got {:?})",
            env.fields.get(k)
        );
    }
    // No extra display fields beyond those expected.
    assert_eq!(
        env.fields.len(),
        expected.as_object().unwrap().len(),
        "{case}: unexpected extra display fields {:?}",
        env.fields
    );
}

fn run_history(case: &str, fx: &Value) {
    let (store, _idx, collection, record) = seed(fx);
    let feed = store.record_history(&collection, &record).unwrap();
    let expected = fx["assert"]["entries"].as_array().expect("entries");
    assert_eq!(feed.len(), expected.len(), "{case}: feed length");
    for (entry, exp) in feed.iter().zip(expected) {
        assert_eq!(entry.version, exp["version"].as_u64().unwrap(), "{case}: version");
        assert_eq!(entry.actor, exp["actor"].as_str().unwrap(), "{case}: actor");
        assert_eq!(
            entry.kind,
            exp["op_kind"].as_str().unwrap(),
            "{case}: op_kind"
        );
        // WHEN: logical_at may be null (a delete).
        let expected_at = exp["logical_at"].as_u64();
        assert_eq!(entry.logical_at, expected_at, "{case}: logical_at");
        // STATE: either the fields, or asserted absent (a tombstone).
        if exp.get("state_absent").and_then(Value::as_bool) == Some(true) {
            assert!(entry.state.is_none(), "{case}: expected tombstone state");
        } else {
            assert_state_fields(case, entry.state.as_ref(), &exp["state_fields"]);
        }
    }
}

fn run_state_at(case: &str, fx: &Value) {
    let (store, _idx, collection, record) = seed(fx);
    for point in fx["assert"]["points"].as_array().expect("points") {
        let version = point["version"].as_u64().unwrap();
        let state = store.record_state_at(&collection, &record, version).unwrap();
        if point.get("state_absent").and_then(Value::as_bool) == Some(true) {
            assert!(state.is_none(), "{case}: v{version} expected absent");
        } else {
            assert_state_fields(case, state.as_ref(), &point["state_fields"]);
        }
    }
}

fn run_restore(case: &str, fx: &Value) {
    let (mut store, idx, collection, record) = seed(fx);
    let a = &fx["assert"];
    let to_version = a["to_version"].as_u64().unwrap();
    let restored_logical_at = a["restored_logical_at"].as_i64();

    // Capture the chunk substrate + feed BEFORE the restore (non-destructive proof).
    let doc_id = collection_doc_id(&collection);
    let chunks_before: Vec<String> = store
        .get_chunks(&doc_id)
        .unwrap()
        .iter()
        .map(|c| c.chunk_id.clone())
        .collect();
    let feed_before = store.record_history(&collection, &record).unwrap();

    let new_version = store
        .restore_record(&collection, &record, to_version, restored_logical_at, &idx)
        .unwrap();
    assert_eq!(
        new_version,
        a["expect_new_version"].as_u64().unwrap(),
        "{case}: new version"
    );

    // Current state after restore.
    let now = store.get_record(&collection, &record).unwrap();
    if a.get("expect_current_absent").and_then(Value::as_bool) == Some(true) {
        assert!(now.is_none(), "{case}: expected current record absent after restore");
    } else {
        assert_state_fields(case, now.as_ref(), &a["expect_current_fields"]);
    }

    // NON-DESTRUCTIVE: every prior chunk remains, plus exactly one new chunk.
    let chunks_after: Vec<String> = store
        .get_chunks(&doc_id)
        .unwrap()
        .iter()
        .map(|c| c.chunk_id.clone())
        .collect();
    for c in &chunks_before {
        assert!(
            chunks_after.contains(c),
            "{case}: prior chunk {c} must remain after restore"
        );
    }
    assert_eq!(
        chunks_after.len(),
        chunks_before.len() + 1,
        "{case}: restore appends exactly one chunk"
    );

    // Prior versions still reconstructable, byte-for-byte.
    if let Some(intact) = a.get("expect_prior_intact").and_then(Value::as_array) {
        for point in intact {
            let v = point["version"].as_u64().unwrap();
            let state = store.record_state_at(&collection, &record, v).unwrap();
            assert_state_fields(case, state.as_ref(), &point["state_fields"]);
        }
    }

    // History grew by the expected amount, and the earlier entries are unchanged.
    let feed_after = store.record_history(&collection, &record).unwrap();
    if let Some(grew) = a.get("expect_history_grew_by").and_then(Value::as_u64) {
        assert_eq!(
            feed_after.len(),
            feed_before.len() + grew as usize,
            "{case}: history growth"
        );
    }
    if let Some(len) = a.get("expect_history_len").and_then(Value::as_u64) {
        assert_eq!(feed_after.len(), len as usize, "{case}: history length");
    }
    assert_eq!(
        &feed_after[..feed_before.len()],
        &feed_before[..],
        "{case}: prior history must be byte-identical after restore (never rewritten)"
    );

    // Optional: a DL-6 rebuild reproduces the restored state.
    if let Some(rebuilt_fields) = a.get("rebuild_then_expect_fields") {
        store.rebuild_projection(&idx).unwrap();
        let rebuilt = store.get_record(&collection, &record).unwrap();
        assert_state_fields(case, rebuilt.as_ref(), rebuilt_fields);
    }
}

fn run_deterministic(case: &str, fx: &Value) {
    let (mut store, idx, collection, record) = seed(fx);
    let a = &fx["assert"];

    // Two history reads are byte-equal (no wall clock in the read path).
    let h1 = store.record_history(&collection, &record).unwrap();
    let h2 = store.record_history(&collection, &record).unwrap();
    assert_eq!(h1, h2, "{case}: history read must be deterministic");

    // A restore replays deterministically: the same op produces the same new version
    // and the same resulting state across a from-scratch rebuild.
    let to_version = a["to_version"].as_u64().unwrap();
    let restored_logical_at = a["restored_logical_at"].as_i64();
    let new_version = store
        .restore_record(&collection, &record, to_version, restored_logical_at, &idx)
        .unwrap();
    assert_eq!(
        new_version,
        a["expect_new_version"].as_u64().unwrap(),
        "{case}: new version"
    );
    assert_state_fields(
        case,
        store.get_record(&collection, &record).unwrap().as_ref(),
        &a["expect_current_fields"],
    );
    // Rebuild from chunks and confirm the same state (replay-identical).
    store.rebuild_projection(&idx).unwrap();
    assert_state_fields(
        case,
        store.get_record(&collection, &record).unwrap().as_ref(),
        &a["expect_current_fields"],
    );
}

fn run_retention(case: &str, fx: &Value) {
    let (mut store, idx, collection, record) = seed(fx);
    let a = &fx["assert"];
    let window = a["window"].as_u64().unwrap();
    let all_peers_acked = a["all_peers_acked"].as_bool().unwrap_or(true);
    assert!(all_peers_acked, "{case}: harness only models the all-peers-acked path");

    let opts = CompactionOptions::all_peers_acked().with_retention(RetentionPolicy::new(window));
    store.compact_history(&opts, &idx).unwrap();

    let doc = collection_doc_id(&collection);
    let op_ids: Vec<String> = store.list_ops().unwrap().into_iter().map(|o| o.op_id).collect();
    for v in a["expect_retained_versions"].as_array().unwrap() {
        let v = v.as_u64().unwrap();
        let op_id = format!("{doc}#chunk-{v:04}");
        assert!(
            op_ids.contains(&op_id),
            "{case}: within-window change-feed entry v{v} ({op_id}) must be retained"
        );
    }
    for v in a["expect_pruned_versions"].as_array().unwrap() {
        let v = v.as_u64().unwrap();
        let op_id = format!("{doc}#chunk-{v:04}");
        assert!(
            !op_ids.contains(&op_id),
            "{case}: beyond-window change-feed entry v{v} ({op_id}) may be pruned"
        );
    }
    // Projection is unchanged by compaction (DL-19 invariant).
    assert_state_fields(
        case,
        store.get_record(&collection, &record).unwrap().as_ref(),
        &a["expect_current_fields"],
    );
}

#[test]
fn dl20_time_travel_fixtures() {
    let manifest = load("manifest.json");
    let cases = manifest["cases"].as_array().expect("manifest cases");
    let declared = manifest["count"].as_u64().expect("manifest count") as usize;
    assert_eq!(cases.len(), declared, "manifest count must match listed cases");

    let mut ran = 0usize;
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let kind = case["kind"].as_str().unwrap();
        let fx = load(case["file"].as_str().unwrap());
        // Sanity: the fixture's manifest kind matches its embedded assert kind.
        assert_eq!(
            fx["assert"]["kind"].as_str().unwrap(),
            kind,
            "{name}: manifest kind must match the fixture assert kind"
        );
        match kind {
            "history" => run_history(name, &fx),
            "state_at" => run_state_at(name, &fx),
            "restore" => run_restore(name, &fx),
            "deterministic" => run_deterministic(name, &fx),
            "retention" => run_retention(name, &fx),
            other => panic!("{name}: unknown assert kind {other}"),
        }
        ran += 1;
    }
    assert_eq!(ran, declared, "every time-travel fixture must run");
}
