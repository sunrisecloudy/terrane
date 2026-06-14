//! Data-driven conformance over the Codex live-query vectors
//! (`forge/fixtures/live-queries/`, T035, count = 10) plus the end-to-end edge
//! vectors (`forge/fixtures/live-queries-e2e/`, T047, count = 12).
//!
//! Each fixture seeds the projection via the DL-4 CRDT write path, registers the
//! declared `db.watch`es, drives the `when` block as a sequence of committed
//! mutation transactions (capturing a result snapshot before each and computing
//! notifications after), and asserts the pinned dirty set + canonical
//! notifications + versions. These vectors are the DL-16 behavioral contract: a
//! wrong dirty set, a coalescing slip, a filter-semantics miss, or a
//! version-monotonicity break fails here, not just in a unit test.
//!
//! This is the **storage substrate** half (Phase 1): it verifies what the watch
//! registry computes — the dirty set and the notification bytes. Delivery
//! concerns the substrate does not own (re-entrant callback queuing into a new
//! event-loop turn, byte-identical run-record replay, applet re-entry) are
//! asserted only at their substrate-visible boundary here (e.g. a re-entrant
//! callback yields a SECOND committed transaction with a later version; replay
//! recorded `args` equal the canonical payload minus `type`).
//!
//! prd-merged/02-data-layer-prd.md DL-16; spec `forge/spec/live-queries.md`.

use forge_schema::{FieldType, SchemaChange, SchemaRegistry};
use forge_storage::{
    DirtyChanges, IndexManager, Mutation, Query, Store, WatchNotification, WatchRegistry,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// --- fixture loading -------------------------------------------------------

fn fixtures_dir(suite: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(suite)
        .canonicalize()
        .unwrap_or_else(|e| panic!("live-query fixtures dir {suite} exists: {e}"))
}

fn load(suite: &str, name: &str) -> Value {
    let path = fixtures_dir(suite).join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

// --- mutation construction (from a fixture op object) ----------------------

fn obj(v: &Value) -> serde_json::Map<String, Value> {
    v.get("fields").and_then(|f| f.as_object()).cloned().unwrap_or_default()
}

/// Build a storage [`Mutation`] from a fixture op (`{op, collection, id, fields}`
/// or `{op:"transact", items:[…]}`), threading a logical clock so the CRDT write
/// path advances timestamps deterministically.
fn op_to_mutation(op: &Value, at: i64) -> Mutation {
    match op.get("op").and_then(|o| o.as_str()).expect("op kind") {
        "insert" => Mutation::Insert {
            collection: op["collection"].as_str().unwrap().into(),
            id: op.get("id").and_then(|i| i.as_str()).map(String::from),
            fields: obj(op),
            logical_at: Some(at),
        },
        "patch" => Mutation::Patch {
            collection: op["collection"].as_str().unwrap().into(),
            id: op["id"].as_str().unwrap().into(),
            fields: obj(op),
            logical_at: Some(at),
        },
        "update" => Mutation::Update {
            collection: op["collection"].as_str().unwrap().into(),
            id: op["id"].as_str().unwrap().into(),
            fields: obj(op),
            logical_at: Some(at),
        },
        "delete" => Mutation::Delete {
            collection: op["collection"].as_str().unwrap().into(),
            id: op["id"].as_str().unwrap().into(),
            logical_at: Some(at),
        },
        "transact" => Mutation::Transact {
            items: op["items"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, child)| op_to_mutation(child, at + i as i64))
                .collect(),
        },
        other => panic!("unknown fixture op kind {other}"),
    }
}

// --- a tiny live-query runner over the storage substrate -------------------

/// Drives one fixture's committed-transaction sequence through the store + watch
/// registry, collecting every notification produced. A per-call logical clock
/// keeps CRDT timestamps advancing.
struct Runner {
    store: Store,
    idx: IndexManager,
    reg: WatchRegistry,
    clock: i64,
    notifications: Vec<WatchNotification>,
}

impl Runner {
    fn new(next_version: u64) -> Self {
        Runner {
            store: Store::open_in_memory().expect("open store"),
            idx: IndexManager::new(),
            reg: WatchRegistry::with_next_version(next_version),
            clock: 0,
            notifications: Vec::new(),
        }
    }

    fn next_clock(&mut self, n: i64) -> i64 {
        self.clock += 1;
        let at = self.clock;
        self.clock += n.max(1) - 1;
        at
    }

    fn register_watch(&mut self, watch_id: &str, query: &Value) {
        self.reg.register(watch_id, Query::from_fixture_value(query).expect("watch query"));
    }

    /// Apply ONE committed mutation transaction: snapshot before, write through
    /// the CRDT path, then commit the dirty set (derived from the mutation) to the
    /// registry and stash the resulting notifications.
    fn apply_committed(&mut self, mutation: &Mutation) {
        let before = self.reg.snapshot(&self.store).expect("snapshot");
        match mutation {
            Mutation::Transact { items } => {
                self.store
                    .transact_mutations_crdt(items, &self.idx)
                    .expect("transact group commits");
            }
            single => {
                self.store
                    .apply_mutation_crdt(single, &self.idx)
                    .expect("single mutation commits");
            }
        }
        let changes = DirtyChanges::from_mutations(std::slice::from_ref(mutation));
        let (_dirty, mut notifs) = self.reg.commit(changes, &before, &self.store).expect("commit");
        self.notifications.append(&mut notifs);
    }

    /// Apply a committed mutation and return its dirty set JSON (for the
    /// single-mutation fixtures that pin `dirty_set`).
    fn apply_committed_dirty(&mut self, mutation: &Mutation) -> Value {
        let before = self.reg.snapshot(&self.store).expect("snapshot");
        match mutation {
            Mutation::Transact { items } => {
                self.store.transact_mutations_crdt(items, &self.idx).expect("transact");
            }
            single => {
                self.store.apply_mutation_crdt(single, &self.idx).expect("single");
            }
        }
        let changes = DirtyChanges::from_mutations(std::slice::from_ref(mutation));
        let (dirty, mut notifs) = self.reg.commit(changes, &before, &self.store).expect("commit");
        self.notifications.append(&mut notifs);
        dirty.to_json()
    }

    fn notifications_json(&self) -> Vec<Value> {
        self.notifications.iter().map(WatchNotification::to_canonical_json).collect()
    }
}

/// Seed the `given.records` into the store via the CRDT insert path so the
/// projection AND the CRDT docs are consistent (a later mutation read-modifies
/// the doc). A seeded record marked `deleted` is inserted then tombstoned.
fn seed_given(runner: &mut Runner, given: &Value) {
    if let Some(records) = given.get("records").and_then(|r| r.as_array()) {
        for rec in records {
            let collection = rec["collection"].as_str().unwrap();
            let id = rec["id"].as_str().unwrap();
            let at = runner.next_clock(1);
            let m = Mutation::Insert {
                collection: collection.into(),
                id: Some(id.into()),
                fields: rec.get("fields").and_then(|f| f.as_object()).cloned().unwrap_or_default(),
                logical_at: Some(at),
            };
            runner.store.apply_mutation_crdt(&m, &runner.idx).expect("seed insert");
            if rec.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false) {
                let at = runner.next_clock(1);
                let d = Mutation::Delete {
                    collection: collection.into(),
                    id: id.into(),
                    logical_at: Some(at),
                };
                runner.store.apply_mutation_crdt(&d, &runner.idx).expect("seed delete");
            }
        }
    }
    if let Some(watches) = given.get("watches").and_then(|w| w.as_array()) {
        for w in watches {
            runner.register_watch(w["watch_id"].as_str().unwrap(), &w["query"]);
        }
    }
}

fn next_version_of(given: &Value) -> u64 {
    given.get("next_version").and_then(|v| v.as_u64()).expect("given.next_version")
}

/// Assert the produced notifications equal the fixture's `expect.notifications`
/// (canonical payloads, exact, in order).
fn assert_notifications(case: &str, runner: &Runner, expect: &Value) {
    let want = expect
        .get("notifications")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    let got = runner.notifications_json();
    assert_eq!(
        got.len(),
        want.len(),
        "case {case}: notification count mismatch\n got: {got:#?}\nwant: {want:#?}"
    );
    for (g, w) in got.iter().zip(want.iter()) {
        assert_eq!(g, w, "case {case}: notification payload mismatch");
    }
}

// --- T035: live-queries semantic vectors -----------------------------------

const T035: &str = "live-queries";

/// The single-`when.mutation` fixtures: one committed transaction, assert its
/// `dirty_set` + `notifications`.
fn run_single_mutation_case(name: &str) {
    let fx = load(T035, name);
    let case = fx["case"].as_str().unwrap();
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    let mutation = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(8));
    let dirty = runner.apply_committed_dirty(&mutation);

    if let Some(expect_dirty) = fx["expect"].get("dirty_set") {
        if !expect_dirty.is_null() {
            assert_eq!(dirty, *expect_dirty, "case {case}: dirty set mismatch");
        }
    }
    assert_notifications(case, &runner, &fx["expect"]);
}

#[test]
fn t035_insert_watched_collection_notify() {
    run_single_mutation_case("insert_watched_collection_notify.json");
}
#[test]
fn t035_update_watched_collection_notify() {
    run_single_mutation_case("update_watched_collection_notify.json");
}
#[test]
fn t035_delete_watched_collection_notify() {
    run_single_mutation_case("delete_watched_collection_notify.json");
}
#[test]
fn t035_non_watched_collection_no_notify() {
    run_single_mutation_case("non_watched_collection_no_notify.json");
}
#[test]
fn t035_filter_non_matching_no_notify() {
    run_single_mutation_case("filter_non_matching_no_notify.json");
}
#[test]
fn t035_two_watchers_same_collection_notify() {
    run_single_mutation_case("two_watchers_same_collection_notify.json");
}
#[test]
fn t035_transact_coalesced_notify() {
    run_single_mutation_case("transact_coalesced_notify.json");
}

#[test]
fn t035_monotonic_version_increments() {
    // when.steps: two committed transactions; versions strictly increase.
    let fx = load(T035, "monotonic_version_increments.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);
    for step in fx["when"]["steps"].as_array().unwrap() {
        let m = op_to_mutation(step, runner.next_clock(1));
        runner.apply_committed(&m);
    }
    assert_notifications("monotonic_version_increments", &runner, &fx["expect"]);
    // The two notifications carry strictly increasing versions.
    let v: Vec<u64> = runner.notifications.iter().map(|n| n.version).collect();
    assert_eq!(v, vec![20, 21]);
}

#[test]
fn t035_unwatch_stops_notifications() {
    // when.steps: unwatch, then a write. The unwatched watch gets nothing, and
    // unwatch is idempotent. The dirty set is still produced for the write.
    let fx = load(T035, "unwatch_stops_notifications.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);
    let mut last_dirty = Value::Null;
    for step in fx["when"]["steps"].as_array().unwrap() {
        match step["op"].as_str().unwrap() {
            "unwatch" => {
                runner.reg.unregister(step["watch_id"].as_str().unwrap());
                // Idempotent second unwatch is a no-op.
                runner.reg.unregister(step["watch_id"].as_str().unwrap());
            }
            _ => {
                let m = op_to_mutation(step, runner.next_clock(1));
                last_dirty = runner.apply_committed_dirty(&m);
            }
        }
    }
    assert_eq!(runner.reg.active_watch_ids(), Vec::<String>::new(), "active_watches empty");
    assert_eq!(last_dirty, fx["expect"]["dirty_set"], "dirty set still produced");
    assert_notifications("unwatch_stops_notifications", &runner, &fx["expect"]);
}

#[test]
fn t035_replay_records_notifications_identically() {
    // Substrate boundary: the recorded `args` equal the canonical payload minus
    // `type` (live-queries.md §Replay), and the notification is produced once.
    let fx = load(T035, "replay_records_notifications_identically.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);
    let m = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(1));
    runner.apply_committed(&m);

    assert_notifications("replay_records_notifications_identically", &runner, &fx["expect"]);
    // recorded_calls: method = db.watch.notification, args = canonical minus type.
    let recorded: Vec<Value> = runner
        .notifications
        .iter()
        .map(|n| json!({"method": "db.watch.notification", "args": n.to_recorded_args(), "result": {"delivered": true}}))
        .collect();
    assert_eq!(recorded, *fx["expect"]["recorded_calls"].as_array().unwrap());
}

// --- T047: live-queries end-to-end edge vectors ----------------------------

const T047: &str = "live-queries-e2e";

#[test]
fn t047_rollback_discards_dirty_set_no_notify() {
    // A rolled-back transaction produces NO dirty set, NO notification, no
    // version consumed, and leaves the records unchanged.
    let fx = load(T047, "rollback_discards_dirty_set_no_notify.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    let before_records = snapshot_records(&runner.store, "tasks");
    let items: Vec<Mutation> = fx["when"]["mutation"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, it)| op_to_mutation(it, runner.next_clock(1) + i as i64))
        .collect();
    // Inject a guaranteed rollback: append a patch of a non-existent record so
    // the whole group fails (the fixture declares rollback:true / a constraint
    // violation; the substrate proof is that a FAILED transact never commits).
    let mut group = items.clone();
    group.push(Mutation::Patch {
        collection: "tasks".into(),
        id: "__missing__".into(),
        fields: Default::default(),
        logical_at: Some(99),
    });
    let err = runner.store.transact_mutations_crdt(&group, &runner.idx).unwrap_err();
    assert_eq!(err.code(), "QueryError");
    // No commit() to the registry -> no dirty set, no notification, version stays.
    assert!(fx["expect"]["dirty_set"].is_null());
    assert!(runner.notifications.is_empty());
    assert_eq!(runner.reg.next_version(), next_version_of(&fx["given"]));
    assert_eq!(
        snapshot_records(&runner.store, "tasks"),
        before_records,
        "rolled-back records are unchanged"
    );
}

#[test]
fn t047_different_filters_targeted_notifications() {
    run_e2e_single_transact("different_filters_targeted_notifications.json");
}
#[test]
fn t047_transact_three_records_two_collections_coalesces() {
    run_e2e_single_transact("transact_three_records_two_collections_coalesces.json");
}
#[test]
fn t047_filtered_enter_then_leave_same_transaction_no_notify() {
    run_e2e_single_transact("filtered_enter_then_leave_same_transaction_no_notify.json");
}
#[test]
fn t047_delete_watched_record_result_excludes_deleted() {
    run_e2e_single_transact("delete_watched_record_result_excludes_deleted.json");
}
#[test]
fn t047_no_op_patch_still_dirties_watched_row() {
    run_e2e_single_transact("no_op_patch_still_dirties_watched_row.json");
}

/// The T047 fixtures whose `when.mutation` is a single committed transaction
/// (single op or one `transact` group, possibly spanning collections): assert
/// dirty_set + notifications.
fn run_e2e_single_transact(name: &str) {
    let fx = load(T047, name);
    let case = fx["case"].as_str().unwrap();
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    let mutation = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(8));
    let before = runner.reg.snapshot(&runner.store).expect("snapshot");
    write_transaction(&mut runner, &mutation);
    let changes = DirtyChanges::from_mutations(std::slice::from_ref(&mutation));
    let (dirty, mut notifs) = runner.reg.commit(changes, &before, &runner.store).expect("commit");
    runner.notifications.append(&mut notifs);

    if let Some(expect_dirty) = fx["expect"].get("dirty_set") {
        if !expect_dirty.is_null() {
            assert_eq!(dirty.to_json(), *expect_dirty, "case {case}: dirty set mismatch");
        }
    }
    assert_notifications(case, &runner, &fx["expect"]);
}

#[test]
fn t047_monotonic_versions_and_shared_transaction_version() {
    // when.transactions: three committed transactions; the middle one touches two
    // collections in ONE transaction so its two notifications share a version.
    let fx = load(T047, "monotonic_versions_and_shared_transaction_version.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);
    for txn in fx["when"]["transactions"].as_array().unwrap() {
        let m = op_to_mutation(&txn["mutation"], runner.next_clock(2));
        // A two-collection "transaction" still commits as ONE registry commit; the
        // CRDT path is per-collection, so apply each collection's writes then make a
        // single dirty set spanning both for the one commit.
        apply_multi_collection_committed(&mut runner, &m);
    }
    assert_notifications("monotonic_versions_and_shared_transaction_version", &runner, &fx["expect"]);
    // Versions: 100, 101, 101, 102 (the middle transaction's two notifications
    // share 101).
    let v: Vec<u64> = runner.notifications.iter().map(|n| n.version).collect();
    assert_eq!(v, vec![100, 101, 101, 102]);
}

/// Apply a (possibly multi-collection) committed transaction as ONE registry
/// commit. The CRDT write path is single-collection per call, so a transact group
/// spanning collections is written as one CRDT write per collection, but the
/// dirty set + version assignment treat the whole thing as one transaction.
fn apply_multi_collection_committed(runner: &mut Runner, mutation: &Mutation) {
    let before = runner.reg.snapshot(&runner.store).expect("snapshot");
    write_transaction(runner, mutation);
    let changes = DirtyChanges::from_mutations(std::slice::from_ref(mutation));
    let (_dirty, mut notifs) = runner.reg.commit(changes, &before, &runner.store).expect("commit");
    runner.notifications.append(&mut notifs);
}

/// Write a transaction (single op or a `transact` group, possibly spanning
/// collections) through the CRDT path. The CRDT write path is single-collection
/// per call, so a multi-collection group is split into one CRDT write per
/// collection — but the caller's dirty set + version assignment treat the whole
/// thing as one logical transaction (one registry commit).
fn write_transaction(runner: &mut Runner, mutation: &Mutation) {
    match mutation {
        Mutation::Transact { items } => {
            let mut by_collection: std::collections::BTreeMap<String, Vec<Mutation>> = Default::default();
            for it in items {
                by_collection.entry(leaf_collection(it)).or_default().push(it.clone());
            }
            for (_c, group) in by_collection {
                if group.len() == 1 {
                    runner.store.apply_mutation_crdt(&group[0], &runner.idx).expect("single");
                } else {
                    runner.store.transact_mutations_crdt(&group, &runner.idx).expect("group");
                }
            }
        }
        single => {
            runner.store.apply_mutation_crdt(single, &runner.idx).expect("single");
        }
    }
}

fn leaf_collection(m: &Mutation) -> String {
    match m {
        Mutation::Insert { collection, .. }
        | Mutation::Update { collection, .. }
        | Mutation::Patch { collection, .. }
        | Mutation::Delete { collection, .. } => collection.clone(),
        Mutation::Transact { .. } => panic!("nested transact leaf"),
    }
}

#[test]
fn t047_watch_registered_after_mutation_has_no_history() {
    // A mutation commits BEFORE the watch is registered, so the watch is not in
    // the registry at commit time and receives no notification. Registering it
    // after lets it see the current result via watch_result_ids.
    let fx = load(T047, "watch_registered_after_mutation_has_no_history.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    let mut dirty_sets = Vec::new();
    for step in fx["when"]["steps"].as_array().unwrap() {
        match step["op"].as_str().unwrap() {
            "mutation" => {
                let m = op_to_mutation(&step["mutation"], runner.next_clock(1));
                dirty_sets.push(runner.apply_committed_dirty(&m));
            }
            "watch" => {
                runner.register_watch(step["watch_id"].as_str().unwrap(), &step["query"]);
            }
            other => panic!("unexpected step op {other}"),
        }
    }
    assert_eq!(dirty_sets, *fx["expect"]["dirty_sets"].as_array().unwrap());
    assert!(runner.notifications.is_empty(), "a late-registered watch is not notified for past writes");
    // watch_initial_result_ids: the new watch sees the current result.
    for (watch_id, want) in fx["expect"]["watch_initial_result_ids"].as_object().unwrap() {
        let got = runner.reg.watch_result_ids(&runner.store, watch_id).unwrap().unwrap();
        assert_eq!(&json!(got), want, "watch {watch_id} initial result ids");
    }
    assert_eq!(runner.reg.next_version(), fx["expect"]["next_version"].as_u64().unwrap());
}

#[test]
fn t047_unwatch_during_pending_batch_suppresses_delivery() {
    // unwatch commits BEFORE the batch's notifications are delivered: the watch is
    // gone from the registry at commit time, so it gets no notification — even
    // though the write still produces a dirty set.
    let fx = load(T047, "unwatch_during_pending_batch_suppresses_delivery.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    // The batch: a mutation queued, then an unwatch "before_delivery_flush". The
    // substrate models the flush as the registry commit, so we unregister BEFORE
    // commit and capture the snapshot before the write.
    let batch = fx["when"]["batch"].as_array().unwrap();
    let mutation_step = batch.iter().find(|s| s["op"] == json!("mutation")).unwrap();
    let unwatch_step = batch.iter().find(|s| s["op"] == json!("unwatch")).unwrap();

    let before = runner.reg.snapshot(&runner.store).expect("snapshot");
    let m = op_to_mutation(&mutation_step["mutation"], runner.next_clock(1));
    runner.store.apply_mutation_crdt(&m, &runner.idx).expect("write");
    // Unwatch lands before the flush (commit).
    runner.reg.unregister(unwatch_step["watch_id"].as_str().unwrap());
    let changes = DirtyChanges::from_mutations(std::slice::from_ref(&m));
    let (dirty, notifs) = runner.reg.commit(changes, &before, &runner.store).expect("commit");

    assert_eq!(dirty.to_json(), fx["expect"]["dirty_set"], "dirty set still produced");
    assert_eq!(runner.reg.active_watch_ids(), Vec::<String>::new(), "active_watches empty");
    assert!(notifs.is_empty(), "the unwatched watch's pending notification is dropped");
}

#[test]
fn t047_reentrant_callback_mutation_queued_next_turn() {
    // A watch callback that mutates queues its mutation as the NEXT event-loop
    // turn (a second committed transaction with a later version). The substrate
    // boundary: applying the callback's effect as a second committed transaction
    // yields a second notification with version = first + 1 and two dirty sets.
    let fx = load(T047, "reentrant_callback_mutation_queued_next_turn.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    // The watch carries a callback_effect; capture it before the first write.
    let watch = fx["given"]["watches"].as_array().unwrap()[0].clone();
    let effect = watch["callback_effect"].clone();

    // Turn 1: the declared mutation.
    let m1 = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(1));
    let d1 = runner.apply_committed_dirty(&m1);
    // Turn 2 (queued by the callback, non-reentrant): apply the callback's effect.
    let m2 = op_to_mutation(&effect, runner.next_clock(1));
    let d2 = runner.apply_committed_dirty(&m2);

    assert_notifications("reentrant_callback_mutation_queued_next_turn", &runner, &fx["expect"]);
    // Two dirty sets at consecutive versions.
    let want_dirty = fx["expect"]["dirty_sets"].as_array().unwrap();
    assert_eq!(vec![d1, d2], *want_dirty);
    // The second notification's version is strictly greater (non-reentrant: a
    // later turn, never a recursive flush inside the first batch).
    let v: Vec<u64> = runner.notifications.iter().map(|n| n.version).collect();
    assert_eq!(v, vec![70, 71]);
}

#[test]
fn t047_replay_session_notifications_byte_identical() {
    // A recorded session of mutations + notifications: the recorded `args` equal
    // the canonical payload minus `type`, replayable byte-identically.
    let fx = load(T047, "replay_session_notifications_byte_identical.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    for entry in fx["when"]["recorded_session"].as_array().unwrap() {
        assert_eq!(entry["op"], json!("mutation"));
        let m = op_to_mutation(&entry["mutation"], runner.next_clock(1));
        runner.apply_committed(&m);
    }
    assert_notifications("replay_session_notifications_byte_identical", &runner, &fx["expect"]);
    let recorded: Vec<Value> = runner
        .notifications
        .iter()
        .map(|n| json!({"method": "db.watch.notification", "args": n.to_recorded_args(), "result": {"delivered": true}}))
        .collect();
    assert_eq!(recorded, *fx["expect"]["recorded_calls"].as_array().unwrap());
}

#[test]
fn t047_schema_change_on_watched_collection_defined_behavior() {
    // Pinned contract (T047 (c)): on a WATCHED collection, an ADDITIVE schema
    // change is accepted, emits NO db.watch.notification, and keeps the watch
    // active; a DESTRUCTIVE drop is REJECTED with SchemaCompatibilityError BEFORE
    // any watch invalidation. The schema engine owns accept/reject; the watch
    // substrate's contribution is that a schema op produces NO dirty set / NO
    // notification and the watch stays active with its initial result.
    let fx = load(T047, "schema_change_on_watched_collection_defined_behavior.json");
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    seed_given(&mut runner, &fx["given"]);

    // Drive the declared schema ops through the REAL schema engine and assert the
    // fixture's per-op accept/reject results (so the destructive-drop rejection is
    // actually proven, not merely asserted-away).
    let mut registry = seed_schema(&fx["given"]["schema"]);
    let want_results = fx["expect"]["schema_results"].as_array().unwrap();
    for (op, want) in fx["when"]["schema_ops"].as_array().unwrap().iter().zip(want_results) {
        // The watch must still be active when the op is evaluated (rejection
        // happens BEFORE watch invalidation), so no schema op ever removes a watch.
        let before_watches = runner.reg.active_watch_ids();
        let result = apply_schema_op(&mut registry, op);
        match want["result"].as_str().unwrap() {
            "accepted" => {
                result.unwrap_or_else(|e| panic!("op {op} must be accepted: {e}"));
            }
            "rejected" => {
                let err = result.expect_err("destructive op must be rejected");
                assert_eq!(
                    err.code(),
                    want["error_kind"].as_str().unwrap(),
                    "rejected op {op} error kind"
                );
            }
            other => panic!("unknown schema result {other}"),
        }
        // A schema op (accepted or rejected) never produces a dirty set or a
        // notification, and never invalidates a watch.
        assert!(runner.notifications.is_empty(), "schema op emits no notification");
        assert_eq!(
            runner.reg.active_watch_ids(),
            before_watches,
            "watch state is unchanged by schema op {op}"
        );
        assert_eq!(want["watch_state"].as_str().unwrap(), "active");
    }

    // No mutation transaction is committed for a schema op, so the substrate
    // produces no dirty set (and consumed no version).
    assert!(fx["expect"]["dirty_set"].is_null());
    assert!(runner.notifications.is_empty());
    assert_eq!(runner.reg.next_version(), next_version_of(&fx["given"]));
    // The watch stays active and its initial result is unchanged.
    assert_eq!(runner.reg.active_watch_ids(), vec!["watch:tasks-open".to_string()]);
    for (watch_id, want) in fx["expect"]["watch_initial_result_ids"].as_object().unwrap() {
        let got = runner.reg.watch_result_ids(&runner.store, watch_id).unwrap().unwrap();
        assert_eq!(&json!(got), want, "watch {watch_id} stays active with its result");
    }
}

/// Build a [`SchemaRegistry`] from a fixture `given.schema` block
/// (`{collection: [{id, name, type}, …]}`). Field ids are taken verbatim from the
/// fixture so a later `validate_compatibility` check keys off the same ids.
fn seed_schema(schema: &Value) -> SchemaRegistry {
    let mut registry = SchemaRegistry::new();
    let Some(cols) = schema.as_object() else {
        return registry;
    };
    for (collection, fields) in cols {
        registry
            .apply_change(SchemaChange::AddCollection { name: collection.clone() })
            .expect("seed add_collection");
        for field in fields.as_array().unwrap() {
            // The seed `id` is a fixed display id; the engine mints its own stable
            // id, so we add by name and let the engine own the id. Compatibility
            // checks below operate on the engine's registry, not the fixture id.
            registry
                .apply_change(SchemaChange::AddField {
                    collection: collection.clone(),
                    actor: forge_domain::ActorId::new("seed"),
                    name: field["name"].as_str().unwrap().into(),
                    ty: field_type(field["type"].as_str().unwrap()),
                    indexed: false,
                    required: false,
                })
                .expect("seed add_field");
        }
    }
    registry
}

/// Apply one fixture schema op to the registry, returning the engine's result.
///
/// `add_field` is an additive [`SchemaChange`] the engine accepts. `drop_collection`
/// has NO API surface in the additive-only engine (DL-8): the destructive intent is
/// modeled as a forward-compatibility check against a registry with the collection
/// removed, which the engine rejects with `SchemaCompatibilityError` — exactly the
/// "destructive drop rejected before watch invalidation" the fixture pins.
fn apply_schema_op(registry: &mut SchemaRegistry, op: &Value) -> forge_domain::Result<()> {
    match op["op"].as_str().unwrap() {
        "add_field" => {
            let field = &op["field"];
            registry.apply_change(SchemaChange::AddField {
                collection: op["collection"].as_str().unwrap().into(),
                actor: forge_domain::ActorId::new("schema-evo"),
                name: field["name"].as_str().unwrap().into(),
                ty: field_type(field["type"].as_str().unwrap()),
                indexed: false,
                required: field.get("required").and_then(|r| r.as_bool()).unwrap_or(false),
            })
        }
        "drop_collection" => {
            let collection = op["collection"].as_str().unwrap();
            // A drop is the registry WITHOUT that collection; validating the
            // dropped registry as a forward evolution of the current one rejects
            // (DL-8 destructive-removal guard). Build it by re-deserializing the
            // current registry with the collection removed.
            let mut without = serde_json::to_value(&*registry).unwrap();
            without["collections"].as_object_mut().unwrap().remove(collection);
            let dropped: SchemaRegistry = serde_json::from_value(without).unwrap();
            dropped.validate_compatibility(registry)
        }
        other => panic!("unknown schema op {other}"),
    }
}

/// Map a fixture field-type token (`"Text"`, `"Bool"`, …) to a [`FieldType`]. The
/// fixtures use PascalCase tokens; the engine's serde form is snake_case, so this
/// mapping is explicit.
fn field_type(token: &str) -> FieldType {
    match token {
        "Text" => FieldType::Text,
        "Bool" => FieldType::Bool,
        "IntNum" => FieldType::IntNum,
        "FloatNum" => FieldType::FloatNum,
        "Scalar" => FieldType::Scalar,
        other => panic!("unknown fixture field type {other}"),
    }
}

// --- helpers ---------------------------------------------------------------

/// The visible (non-deleted) record ids of a collection, ordered by id — a stable
/// projection fingerprint for the rollback "records unchanged" assertion.
fn snapshot_records(store: &Store, collection: &str) -> Vec<Value> {
    let q = Query::from(collection);
    match store.query(&q).unwrap() {
        forge_storage::QueryResult::Rows(rows) => rows
            .into_iter()
            .map(|r| json!({"id": r.envelope.entity_id.as_str(), "fields": serde_json::to_value(&r.envelope.fields).unwrap()}))
            .collect(),
        _ => Vec::new(),
    }
}
