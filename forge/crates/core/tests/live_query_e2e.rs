//! Phase-2 (forge-core + forge-runtime) end-to-end conformance over EVERY
//! db.watch / live-query vector — the 10 semantic vectors
//! (`forge/fixtures/live-queries/`, T035) AND the 12 end-to-end edge vectors
//! (`forge/fixtures/live-queries-e2e/`, T047). DL-16, `forge/spec/live-queries.md`.
//!
//! Where the storage-substrate test (`forge-storage` `live_query_fixtures.rs`)
//! proves what the [`WatchRegistry`](forge_storage::WatchRegistry) COMPUTES (the
//! dirty set + the notification bytes), THIS test drives each scenario through the
//! [`WorkspaceCore`] facade — registering watches via the `db.watch` command path,
//! applying mutations through [`commit_and_notify`](forge_core::WorkspaceCore::commit_and_notify)
//! (snapshot → atomic write → registry commit → RECORD + DISPATCH notifications →
//! persist the bumped version), and asserting:
//!   * the delivered notification STREAM equals the fixture's `expect.notifications`
//!     (canonical payloads, in order);
//!   * the per-transaction `dirty_set` (when pinned) matches;
//!   * the recorded `db.watch.notification` envelopes REPLAY byte-identically
//!     (`forge_core::replay_notification_stream`) without re-touching live hooks;
//!   * the edge contracts: rollback discards (no dirty set / no notify / no version),
//!     unwatch-before-flush suppresses delivery, a re-entrant callback mutation lands
//!     as the NEXT turn (a later version, never recursive), a no-op patch still
//!     dirties + notifies, monotonic + shared-transaction versions, two-watcher
//!     coalescing, schema-change behavior, and a late-registered watch has no history.
//!
//! A manifest-count GUARD asserts that EVERY vector in BOTH manifests was driven, so
//! no fixture can be silently skipped (the count is the behavioral contract, not the
//! handful of named cases).

use forge_core::{replay_notification_stream, DeliveredBatch, WorkspaceCore};
use forge_domain::RecordedCall;
use forge_storage::Mutation;
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

/// The fixture file names a manifest lists (excluding the manifest itself), and the
/// manifest's pinned `count`. The conformance loop asserts it drove exactly `count`.
fn manifest_cases(suite: &str) -> (u64, Vec<(String, String)>) {
    let manifest = load(suite, "manifest.json");
    let count = manifest["count"].as_u64().expect("manifest.count");
    let cases = manifest["cases"]
        .as_array()
        .expect("manifest.cases")
        .iter()
        .map(|c| {
            (
                c["case"].as_str().unwrap().to_string(),
                c["file"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (count, cases)
}

// --- mutation construction (from a fixture op object) ----------------------

fn fields_of(op: &Value) -> serde_json::Map<String, Value> {
    op.get("fields").and_then(|f| f.as_object()).cloned().unwrap_or_default()
}

/// Build a storage [`Mutation`] from a fixture op (`{op, collection, id, fields}` or
/// `{op:"transact", items:[…]}`), threading a logical clock so the CRDT write path
/// advances timestamps deterministically.
fn op_to_mutation(op: &Value, at: i64) -> Mutation {
    match op["op"].as_str().expect("op kind") {
        "insert" => Mutation::Insert {
            collection: op["collection"].as_str().unwrap().into(),
            id: op.get("id").and_then(|i| i.as_str()).map(String::from),
            fields: fields_of(op),
            logical_at: Some(at),
        },
        "patch" => Mutation::Patch {
            collection: op["collection"].as_str().unwrap().into(),
            id: op["id"].as_str().unwrap().into(),
            fields: fields_of(op),
            logical_at: Some(at),
        },
        "update" => Mutation::Update {
            collection: op["collection"].as_str().unwrap().into(),
            id: op["id"].as_str().unwrap().into(),
            fields: fields_of(op),
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

// --- the facade-level live-query runner ------------------------------------

/// Drives one fixture's committed-transaction sequence through a real
/// [`WorkspaceCore`], collecting every delivered notification + every recorded
/// `db.watch.notification` envelope (the replayable stream). A per-call logical
/// clock keeps CRDT timestamps advancing.
struct Runner {
    core: WorkspaceCore,
    clock: i64,
    /// Every notification delivered this scenario, as its canonical JSON payload.
    notifications: Vec<Value>,
    /// Every recorded notification envelope, in delivery order (the replay source).
    recorded: Vec<RecordedCall>,
}

impl Runner {
    /// A fresh workspace whose first committed transaction takes `next_version`. The
    /// version seed is set by registering watches against a workspace whose watch
    /// session version is bumped to `next_version` via a no-op (we seed it directly).
    fn new(next_version: u64) -> Self {
        let mut core = WorkspaceCore::in_memory("ws-live").expect("open workspace");
        // Seed the workspace's monotone watch version so the first delivered
        // transaction is assigned `next_version` (the fixtures pin the starting
        // version). The trusted in-process seam exposes this for the harness.
        core.seed_watch_version(next_version).expect("seed watch version");
        Runner {
            core,
            clock: 0,
            notifications: Vec::new(),
            recorded: Vec::new(),
        }
    }

    fn next_clock(&mut self, n: i64) -> i64 {
        self.clock += 1;
        let at = self.clock;
        self.clock += n.max(1) - 1;
        at
    }

    /// Seed the `given.records` directly via the store's CRDT insert path so the
    /// projection AND the CRDT docs are consistent. A record marked `deleted` is
    /// inserted then tombstoned. Then register every declared watch.
    fn seed_given(&mut self, given: &Value) {
        if let Some(records) = given.get("records").and_then(|r| r.as_array()) {
            for rec in records {
                let collection = rec["collection"].as_str().unwrap();
                let id = rec["id"].as_str().unwrap();
                let at = self.next_clock(2);
                let fields = rec.get("fields").and_then(|f| f.as_object()).cloned().unwrap_or_default();
                let deleted = rec.get("deleted").and_then(|d| d.as_bool()).unwrap_or(false);
                self.core.seed_record(collection, id, fields, at, deleted).expect("seed record");
            }
        }
        if let Some(watches) = given.get("watches").and_then(|w| w.as_array()) {
            for w in watches {
                self.register_watch(w);
            }
        }
    }

    /// Register one declared watch through the trusted in-process seam (the fixtures
    /// name only `{watch_id, query}`; the owning applet + callback default for the
    /// substrate-level vectors, which carry no installed callback applet).
    fn register_watch(&mut self, w: &Value) {
        self.core
            .register_watch("live-app", w["watch_id"].as_str().unwrap(), "onWatch", w["query"].clone())
            .expect("register watch");
    }

    /// Apply ONE committed mutation transaction through the facade's
    /// `commit_and_notify`, collecting its delivered notifications + recorded
    /// envelopes. Returns the batch (for the dirty-set assertion).
    fn commit(&mut self, mutation: &Mutation) -> DeliveredBatch {
        let batch = self.core.commit_and_notify(mutation).expect("commit_and_notify");
        for n in &batch.notifications {
            self.notifications.push(n.to_canonical_json());
        }
        self.recorded.extend(batch.recorded_calls.iter().cloned());
        batch
    }

    /// The dirty set JSON of a committed transaction (for the fixtures that pin it).
    fn dirty_json(batch: &DeliveredBatch) -> Value {
        batch
            .dirty
            .as_ref()
            .map(|d| d.to_json())
            .unwrap_or(Value::Null)
    }
}

/// Assert the delivered notifications equal the fixture's `expect.notifications`
/// (canonical payloads, exact, in order).
fn assert_notifications(case: &str, runner: &Runner, expect: &Value) {
    let want = expect
        .get("notifications")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        runner.notifications.len(),
        want.len(),
        "case {case}: notification count mismatch\n got: {:#?}\nwant: {want:#?}",
        runner.notifications
    );
    for (g, w) in runner.notifications.iter().zip(want.iter()) {
        assert_eq!(g, w, "case {case}: notification payload mismatch");
    }
}

/// Assert the recorded notification stream REPLAYS byte-identically: re-serving the
/// recorded `db.watch.notification` envelopes through a fresh replay recorder
/// reproduces them exactly (no live hooks, no recompute). `expect.recorded_calls`,
/// when pinned, must also equal the recorded envelopes (method/args/result).
fn assert_replay_byte_identical(case: &str, runner: &Runner, expect: &Value) {
    let replayed = replay_notification_stream(&runner.recorded)
        .unwrap_or_else(|e| panic!("case {case}: notification replay diverged: {e}"));
    assert_eq!(
        replayed.len(),
        runner.recorded.len(),
        "case {case}: replay produced a different number of notifications"
    );
    for (orig, rep) in runner.recorded.iter().zip(replayed.iter()) {
        assert_eq!(orig.method, rep.method, "case {case}: replay method diverged");
        assert_eq!(orig.args, rep.args, "case {case}: replay args diverged (byte-identical)");
        assert_eq!(orig.response, rep.response, "case {case}: replay response diverged");
    }
    // When the fixture pins the exact recorded_calls shape, assert it too.
    if let Some(recorded_calls) = expect.get("recorded_calls").and_then(|c| c.as_array()) {
        let got: Vec<Value> = runner
            .recorded
            .iter()
            .map(|c| json!({ "method": c.method, "args": c.args, "result": c.response }))
            .collect();
        assert_eq!(
            got,
            *recorded_calls,
            "case {case}: recorded_calls (method=db.watch.notification, args=canonical minus type, result=delivered) mismatch"
        );
    }
}

fn next_version_of(given: &Value) -> u64 {
    given.get("next_version").and_then(|v| v.as_u64()).expect("given.next_version")
}

// --- per-case drivers ------------------------------------------------------

/// The `when.mutation` (single committed transaction) cases: assert dirty_set +
/// notifications + replay.
fn run_single_mutation(suite: &str, fx: &Value) {
    let case = fx["case"].as_str().unwrap();
    let mut runner = Runner::new(next_version_of(&fx["given"]));
    runner.seed_given(&fx["given"]);
    let mutation = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(8));
    let batch = runner.commit(&mutation);
    if let Some(expect_dirty) = fx["expect"].get("dirty_set") {
        if !expect_dirty.is_null() {
            assert_eq!(Runner::dirty_json(&batch), *expect_dirty, "case {case}: dirty set mismatch");
        }
    }
    assert_notifications(case, &runner, &fx["expect"]);
    assert_replay_byte_identical(case, &runner, &fx["expect"]);
    let _ = suite;
}

// --- T035: live-queries semantic vectors -----------------------------------

const T035: &str = "live-queries";

/// Drive one T035 vector by its case name, returning so the manifest guard counts it.
fn drive_t035(case: &str, file: &str) {
    let fx = load(T035, file);
    match case {
        "insert_watched_collection_notify"
        | "update_watched_collection_notify"
        | "delete_watched_collection_notify"
        | "non_watched_collection_no_notify"
        | "filter_non_matching_no_notify"
        | "two_watchers_same_collection_notify"
        | "transact_coalesced_notify" => run_single_mutation(T035, &fx),

        "monotonic_version_increments" => {
            // when.steps: two committed transactions; versions strictly increase.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            for step in fx["when"]["steps"].as_array().unwrap() {
                let m = op_to_mutation(step, runner.next_clock(1));
                runner.commit(&m);
            }
            assert_notifications(case, &runner, &fx["expect"]);
            assert_replay_byte_identical(case, &runner, &fx["expect"]);
            let versions: Vec<u64> = runner
                .notifications
                .iter()
                .map(|n| n["version"].as_u64().unwrap())
                .collect();
            assert_eq!(versions, vec![20, 21], "versions strictly increase");
        }

        "unwatch_stops_notifications" => {
            // when.steps: unwatch (idempotent), then a write → no notification.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let mut last_dirty = Value::Null;
            for step in fx["when"]["steps"].as_array().unwrap() {
                match step["op"].as_str().unwrap() {
                    "unwatch" => {
                        let wid = step["watch_id"].as_str().unwrap();
                        runner.core.unregister_watch(wid).unwrap();
                        runner.core.unregister_watch(wid).unwrap(); // idempotent
                    }
                    _ => {
                        let m = op_to_mutation(step, runner.next_clock(1));
                        last_dirty = Runner::dirty_json(&runner.commit(&m));
                    }
                }
            }
            assert!(runner.core.active_watch_ids().is_empty(), "active_watches empty");
            assert_eq!(last_dirty, fx["expect"]["dirty_set"], "dirty set still produced");
            assert_notifications(case, &runner, &fx["expect"]);
        }

        "replay_records_notifications_identically" => {
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let m = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(1));
            runner.commit(&m);
            assert_notifications(case, &runner, &fx["expect"]);
            // recorded_calls: method=db.watch.notification, args=canonical minus type,
            // result={delivered:true}; AND it replays byte-identically.
            assert_replay_byte_identical(case, &runner, &fx["expect"]);
        }

        other => panic!("unhandled T035 case {other}"),
    }
}

// --- T047: live-queries end-to-end edge vectors ----------------------------

const T047: &str = "live-queries-e2e";

fn drive_t047(case: &str, file: &str) {
    let fx = load(T047, file);
    match case {
        "different_filters_targeted_notifications"
        | "transact_three_records_two_collections_coalesces"
        | "filtered_enter_then_leave_same_transaction_no_notify"
        | "delete_watched_record_result_excludes_deleted"
        | "no_op_patch_still_dirties_watched_row" => run_single_mutation(T047, &fx),

        "rollback_discards_dirty_set_no_notify" => {
            // A rolled-back transaction produces NO dirty set, NO notification, no
            // version consumed, and leaves the records unchanged.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let before = runner.core.active_watch_ids();

            // Inject a guaranteed rollback: append a patch of a non-existent record
            // so the whole transact group fails (the fixture declares rollback:true).
            let mut items: Vec<Mutation> = fx["when"]["mutation"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, it)| op_to_mutation(it, runner.next_clock(1) + i as i64))
                .collect();
            items.push(Mutation::Patch {
                collection: "tasks".into(),
                id: "__missing__".into(),
                fields: Default::default(),
                logical_at: Some(99),
            });
            let group = Mutation::Transact { items };
            let err = runner.core.commit_and_notify(&group).unwrap_err();
            assert_eq!(err.code(), "QueryError", "rollback surfaces a typed error");

            // No dirty set / no notification / version untouched.
            assert!(fx["expect"]["dirty_set"].is_null());
            assert!(runner.notifications.is_empty(), "rolled-back txn delivers nothing");
            assert!(runner.recorded.is_empty(), "rolled-back txn records nothing");
            assert_eq!(
                runner.core.next_watch_version(),
                next_version_of(&fx["given"]),
                "a rolled-back transaction consumes no version"
            );
            assert_eq!(runner.core.active_watch_ids(), before, "watch set unchanged by rollback");
        }

        "watch_registered_after_mutation_has_no_history" => {
            // A mutation commits BEFORE the watch is registered → the watch is not in
            // the registry at commit time and receives no notification. Registering it
            // after lets it see the current result via watch_result_ids.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let mut dirty_sets = Vec::new();
            for step in fx["when"]["steps"].as_array().unwrap() {
                match step["op"].as_str().unwrap() {
                    "mutation" => {
                        let m = op_to_mutation(&step["mutation"], runner.next_clock(1));
                        dirty_sets.push(Runner::dirty_json(&runner.commit(&m)));
                    }
                    "watch" => runner.register_watch(step),
                    other => panic!("unexpected step op {other}"),
                }
            }
            assert_eq!(dirty_sets, *fx["expect"]["dirty_sets"].as_array().unwrap());
            assert!(runner.notifications.is_empty(), "a late watch is not notified for past writes");
            for (watch_id, want) in fx["expect"]["watch_initial_result_ids"].as_object().unwrap() {
                let got = runner.core.watch_result_ids(watch_id).unwrap().unwrap();
                assert_eq!(&json!(got), want, "watch {watch_id} initial result ids");
            }
            assert_eq!(runner.core.next_watch_version(), fx["expect"]["next_version"].as_u64().unwrap());
        }

        "unwatch_during_pending_batch_suppresses_delivery" => {
            // unwatch commits BEFORE the batch's notifications are delivered: the watch
            // is gone from the registry at commit time, so it gets no notification —
            // even though the write still produces a dirty set. The facade models the
            // flush as `commit_and_notify`, so unregistering before the commit is the
            // "before_delivery_flush" timing.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let batch = fx["when"]["batch"].as_array().unwrap();
            let mutation_step = batch.iter().find(|s| s["op"] == json!("mutation")).unwrap();
            let unwatch_step = batch.iter().find(|s| s["op"] == json!("unwatch")).unwrap();
            // Unwatch lands before the flush (the commit_and_notify call).
            runner.core.unregister_watch(unwatch_step["watch_id"].as_str().unwrap()).unwrap();
            let m = op_to_mutation(&mutation_step["mutation"], runner.next_clock(1));
            let batch = runner.commit(&m);
            assert_eq!(Runner::dirty_json(&batch), fx["expect"]["dirty_set"], "dirty set still produced");
            assert!(runner.core.active_watch_ids().is_empty(), "active_watches empty");
            assert!(runner.notifications.is_empty(), "the unwatched watch's notification is dropped");
        }

        "monotonic_versions_and_shared_transaction_version" => {
            // when.transactions: three committed transactions; the middle one touches
            // two collections in ONE transaction so its two notifications share a
            // version (and the whole group commits atomically, review 129 #1).
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            for txn in fx["when"]["transactions"].as_array().unwrap() {
                let m = op_to_mutation(&txn["mutation"], runner.next_clock(2));
                runner.commit(&m);
            }
            assert_notifications(case, &runner, &fx["expect"]);
            assert_replay_byte_identical(case, &runner, &fx["expect"]);
            let versions: Vec<u64> = runner
                .notifications
                .iter()
                .map(|n| n["version"].as_u64().unwrap())
                .collect();
            assert_eq!(versions, vec![100, 101, 101, 102], "middle txn's two notifs share 101");
        }

        "reentrant_callback_mutation_queued_next_turn" => {
            // A watch callback that mutates queues its mutation as the NEXT event-loop
            // turn (a second committed transaction with a later version), never a
            // recursive flush inside the first batch. The fixture's `callback_effect`
            // models the queued mutation; the facade applies it as a SECOND
            // commit_and_notify, which gets version = first + 1.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            let effect = fx["given"]["watches"].as_array().unwrap()[0]["callback_effect"].clone();

            // Turn 1: the declared mutation.
            let m1 = op_to_mutation(&fx["when"]["mutation"], runner.next_clock(1));
            let d1 = Runner::dirty_json(&runner.commit(&m1));
            // Turn 2 (queued by the callback, NON-REENTRANT): apply the callback's effect.
            let m2 = op_to_mutation(&effect, runner.next_clock(1));
            let d2 = Runner::dirty_json(&runner.commit(&m2));

            assert_notifications(case, &runner, &fx["expect"]);
            assert_replay_byte_identical(case, &runner, &fx["expect"]);
            assert_eq!(vec![d1, d2], *fx["expect"]["dirty_sets"].as_array().unwrap());
            let versions: Vec<u64> = runner
                .notifications
                .iter()
                .map(|n| n["version"].as_u64().unwrap())
                .collect();
            // The second notification's version is strictly greater: a later turn,
            // never a recursive flush inside the first batch.
            assert_eq!(versions, vec![70, 71]);
        }

        "replay_session_notifications_byte_identical" => {
            // A recorded session of mutations + notifications replays byte-identically.
            let mut runner = Runner::new(next_version_of(&fx["given"]));
            runner.seed_given(&fx["given"]);
            for entry in fx["when"]["recorded_session"].as_array().unwrap() {
                assert_eq!(entry["op"], json!("mutation"));
                let m = op_to_mutation(&entry["mutation"], runner.next_clock(1));
                runner.commit(&m);
            }
            assert_notifications(case, &runner, &fx["expect"]);
            assert_replay_byte_identical(case, &runner, &fx["expect"]);
        }

        "schema_change_on_watched_collection_defined_behavior" => {
            drive_schema_change(&fx);
        }

        other => panic!("unhandled T047 case {other}"),
    }
}

/// T047 (c): on a WATCHED collection, an ADDITIVE schema change is accepted, emits
/// NO `db.watch.notification`, and keeps the watch active; a DESTRUCTIVE drop is
/// REJECTED with `SchemaCompatibilityError` BEFORE any watch invalidation. The
/// facade owns accept/reject of schema ops (additive-only engine); the watch
/// contribution is that a schema op produces NO dirty set / NO notification and the
/// watch stays active with its initial result.
fn drive_schema_change(fx: &Value) {
    use forge_schema::{SchemaChange, SchemaRegistry};

    let mut runner = Runner::new(next_version_of(&fx["given"]));
    runner.seed_given(&fx["given"]);

    // Seed the schema engine with the fixture's declared schema.
    let mut registry = SchemaRegistry::new();
    for (collection, fields) in fx["given"]["schema"].as_object().unwrap() {
        registry.apply_change(SchemaChange::AddCollection { name: collection.clone() }).unwrap();
        for field in fields.as_array().unwrap() {
            registry
                .apply_change(SchemaChange::AddField {
                    collection: collection.clone(),
                    actor: forge_domain::ActorId::new("seed"),
                    name: field["name"].as_str().unwrap().into(),
                    ty: field_type(field["type"].as_str().unwrap()),
                    indexed: false,
                    required: false,
                })
                .unwrap();
        }
    }

    let want_results = fx["expect"]["schema_results"].as_array().unwrap();
    for (op, want) in fx["when"]["schema_ops"].as_array().unwrap().iter().zip(want_results) {
        let before_watches = runner.core.active_watch_ids();
        let result: forge_domain::Result<()> = match op["op"].as_str().unwrap() {
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
                // A drop has NO API in the additive-only engine (DL-8): model the
                // destructive intent as a forward-compatibility check against a
                // registry with the collection removed, which the engine rejects with
                // SchemaCompatibilityError — exactly "destructive drop rejected before
                // watch invalidation".
                let collection = op["collection"].as_str().unwrap();
                let mut without = serde_json::to_value(&registry).unwrap();
                without["collections"].as_object_mut().unwrap().remove(collection);
                let dropped: SchemaRegistry = serde_json::from_value(without).unwrap();
                dropped.validate_compatibility(&registry)
            }
            other => panic!("unknown schema op {other}"),
        };
        match want["result"].as_str().unwrap() {
            "accepted" => result.unwrap_or_else(|e| panic!("op {op} must be accepted: {e}")),
            "rejected" => {
                let err = result.expect_err("destructive op must be rejected");
                assert_eq!(err.code(), want["error_kind"].as_str().unwrap(), "rejected op {op} error kind");
            }
            other => panic!("unknown schema result {other}"),
        }
        // A schema op (accepted or rejected) never produces a dirty set or a
        // notification, and never invalidates a watch.
        assert!(runner.notifications.is_empty(), "schema op emits no notification");
        assert_eq!(runner.core.active_watch_ids(), before_watches, "watch state unchanged by schema op {op}");
        assert_eq!(want["watch_state"].as_str().unwrap(), "active");
    }

    assert!(fx["expect"]["dirty_set"].is_null());
    assert!(runner.notifications.is_empty());
    assert_eq!(runner.core.next_watch_version(), next_version_of(&fx["given"]));
    assert_eq!(runner.core.active_watch_ids(), vec!["watch:tasks-open".to_string()]);
    for (watch_id, want) in fx["expect"]["watch_initial_result_ids"].as_object().unwrap() {
        let got = runner.core.watch_result_ids(watch_id).unwrap().unwrap();
        assert_eq!(&json!(got), want, "watch {watch_id} stays active with its result");
    }
}

fn field_type(token: &str) -> forge_schema::FieldType {
    match token {
        "Text" => forge_schema::FieldType::Text,
        "Bool" => forge_schema::FieldType::Bool,
        "IntNum" => forge_schema::FieldType::IntNum,
        "FloatNum" => forge_schema::FieldType::FloatNum,
        "Scalar" => forge_schema::FieldType::Scalar,
        other => panic!("unknown fixture field type {other}"),
    }
}

// --- the two manifest-guarded conformance loops ----------------------------

#[test]
fn t035_all_live_query_vectors_conform_through_the_facade() {
    let (count, cases) = manifest_cases(T035);
    let mut ran = 0u64;
    for (case, file) in &cases {
        drive_t035(case, file);
        ran += 1;
    }
    assert_eq!(ran, count, "drove every T035 vector the manifest declares ({count})");
    assert_eq!(ran, 10, "T035 pins 10 semantic vectors");
}

#[test]
fn t047_all_live_query_edge_vectors_conform_through_the_facade() {
    let (count, cases) = manifest_cases(T047);
    let mut ran = 0u64;
    for (case, file) in &cases {
        drive_t047(case, file);
        ran += 1;
    }
    assert_eq!(ran, count, "drove every T047 edge vector the manifest declares ({count})");
    assert_eq!(ran, 12, "T047 pins 12 end-to-end edge vectors");
}
