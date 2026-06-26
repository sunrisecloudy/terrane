//! DL-20 file-level time travel, exercised through the LIVE command surface
//! (`WorkspaceCore::handle`) — the `db.history` + `db.restore` commands, not the raw
//! `Store::record_history` / `Store::restore_record` the `forge-storage` harness
//! (`time_travel_fixtures.rs`) covers. These tests prove the COMMAND boundary the
//! storage layer defers to (`forge/spec/time-travel.md` §5):
//!
//!   - `db.history` returns the change feed (WHO/WHEN/WHAT + state, in version order),
//!     gated by the collection-scoped `db.read` capability;
//!   - `db.restore` appends a NEW version equal to a prior state — never a destructive
//!     rollback — gated by the collection-scoped `db.write` capability, and the prior
//!     history stays byte-intact;
//!   - retention compaction (DL-20 §4) keeps the within-window change-feed entries and
//!     prunes beyond, visible through `db.history` after compaction.
//!
//! Data-driven over EVERY `forge/fixtures/time-travel/*.json` (skipping `manifest.json`):
//! the manifest enumerates the cases, each fixture seeds a record through the real
//! DL-4 CRDT mutation path, and the harness drives the matching command(s) and asserts
//! the result. A `ran == manifest.count` guard makes a silently-skipped case fail.
//!
//! `live_*` tests below additionally prove the gate is LIVE-WIRED (not just present):
//! a `db.restore` drives the real command path and a `db.read`-denied `db.history` is
//! rejected with the right error code.

use forge_core::WorkspaceCore;
use forge_domain::{
    ActorContext, AppletId, CoreCommand, CoreResponse, RequestId, Role, WorkspaceId,
};
use forge_storage::{CompactionOptions, IndexManager, Mutation, RetentionPolicy};
use serde_json::{json, Value};

fn fixtures_dir() -> std::path::PathBuf {
    // crates/core/tests/ -> ../../../fixtures/time-travel
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

/// The owner actor every fixture drives commands as. An owner with NO trusted grant
/// entry falls back to its role-derived read/write-all scope, so the conformance
/// vectors (which provision no grants) exercise the happy path; the `live_*` tests
/// pin a narrowed scope to prove the gate denies.
fn owner() -> ActorContext {
    ActorContext::owner("dev")
}

fn cmd(actor: ActorContext, name: &str, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req-tt"),
        name: name.into(),
        applet_id: None::<AppletId>,
        actor,
        workspace_id: WorkspaceId::new("ws"),
        payload,
    }
}

/// Seed one mutation through the real DL-4 CRDT write path (`apply_mutation_crdt`),
/// the SAME path the live spine and the storage harness use. Seeding through the
/// store (not a command) is deliberate: there is no top-level "write a record"
/// command in M0a (records are written via `ctx.db.*` inside a run), and the point
/// of THIS harness is the `db.history`/`db.restore` command surface over a populated
/// history.
fn apply_seed(core: &mut WorkspaceCore, idx: &IndexManager, collection: &str, m: &Value) {
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
        "delete" => Mutation::Delete { collection: collection.into(), id, logical_at },
        other => panic!("unknown seed op {other}"),
    };
    core.store_mut()
        .apply_mutation_crdt(&mutation, idx)
        .expect("seed mutation");
}

/// Seed a fresh workspace from the fixture's `seed` array, returning it plus the
/// collection/record names. The index manager is created fresh and used only for the
/// seed writes (the live commands read `core.indexes()` internally).
fn seed(fx: &Value) -> (WorkspaceCore, String, String) {
    let collection = fx["collection"].as_str().expect("collection").to_string();
    let record = fx["record"].as_str().expect("record").to_string();
    let mut core = WorkspaceCore::in_memory("ws-time-travel").unwrap();
    let idx = IndexManager::new();
    for m in fx["seed"].as_array().expect("seed array") {
        apply_seed(&mut core, &idx, &collection, m);
    }
    (core, collection, record)
}

/// Drive `db.history` through the live command path, returning the `entries` array.
fn history(core: &mut WorkspaceCore, actor: ActorContext, collection: &str, id: &str) -> CoreResponse {
    core.handle(cmd(
        actor,
        "db.history",
        json!({ "collection": collection, "id": id }),
    ))
}

/// The `entries` array from a successful `db.history` response.
fn entries(resp: &CoreResponse) -> Vec<Value> {
    assert!(resp.ok, "db.history should succeed: {:?}", resp.error);
    resp.payload["entries"].as_array().expect("entries array").clone()
}

/// Assert a state object's display `fields` equal the fixture's `state_fields`.
fn assert_state_fields(case: &str, state: &Value, expected: &Value) {
    let fields = state["fields"].as_object().unwrap_or_else(|| {
        panic!("{case}: expected a record state with fields, got {state}")
    });
    for (k, v) in expected.as_object().expect("state_fields object") {
        assert_eq!(fields.get(k), Some(v), "{case}: field {k} mismatch (got {:?})", fields.get(k));
    }
    assert_eq!(
        fields.len(),
        expected.as_object().unwrap().len(),
        "{case}: unexpected extra display fields {fields:?}"
    );
}

/// `history` kind: `db.history` lists the who/when/what + state entries in version
/// order.
fn run_history(case: &str, fx: &Value) {
    let (mut core, collection, record) = seed(fx);
    let resp = history(&mut core, owner(), &collection, &record);
    let feed = entries(&resp);
    let expected = fx["assert"]["entries"].as_array().expect("entries");
    assert_eq!(feed.len(), expected.len(), "{case}: feed length");
    for (entry, exp) in feed.iter().zip(expected) {
        assert_eq!(entry["version"], exp["version"], "{case}: version");
        assert_eq!(entry["actor"], exp["actor"], "{case}: actor");
        assert_eq!(entry["kind"], exp["op_kind"], "{case}: op_kind");
        // WHEN: logical_at is the envelope's updated_at, or — for a delete (no surviving
        // envelope) — the delete's own mutation timestamp recovered from the oplog row
        // (DL-20 review 169). `null` only for a delete row carrying no mutation timestamp.
        let expected_at = exp["logical_at"].clone();
        assert_eq!(entry["logical_at"], expected_at, "{case}: logical_at");
        // STATE: the fields, or asserted absent (a tombstone => null state).
        if exp.get("state_absent").and_then(Value::as_bool) == Some(true) {
            assert!(entry["state"].is_null(), "{case}: expected tombstone (null) state");
        } else {
            assert_state_fields(case, &entry["state"], &exp["state_fields"]);
        }
    }
}

/// `state_at` kind: each listed point's state is reflected by the change feed (the
/// per-version `state` `db.history` returns IS `record_state_at`). A point with no
/// matching feed entry (version 0, or a version past the last change) is checked
/// against the nearest preceding entry's state — the feed carries the same state the
/// storage `record_state_at` reconstructs.
fn run_state_at(case: &str, fx: &Value) {
    let (mut core, collection, record) = seed(fx);
    let resp = history(&mut core, owner(), &collection, &record);
    let feed = entries(&resp);
    for point in fx["assert"]["points"].as_array().expect("points") {
        let version = point["version"].as_u64().unwrap();
        // The feed entry whose version is the greatest <= the asked version is the
        // record's state AS OF that version (later entries have not happened yet).
        let at = feed
            .iter()
            .rfind(|e| e["version"].as_u64().unwrap() <= version);
        if point.get("state_absent").and_then(Value::as_bool) == Some(true) {
            // Absent: either no entry <= version, or that entry tombstoned the record.
            let absent = match at {
                None => true,
                Some(e) => e["state"].is_null(),
            };
            assert!(absent, "{case}: v{version} expected absent");
        } else {
            let e = at.unwrap_or_else(|| panic!("{case}: v{version} expected a state, none in feed"));
            assert_state_fields(case, &e["state"], &point["state_fields"]);
        }
    }
}

/// Drive `db.restore` through the live command path, asserting the new version,
/// current state, and the NON-DESTRUCTIVE contract (prior history byte-intact, feed
/// grew). Shared by the `restore` and `deterministic` kinds.
fn drive_restore(core: &mut WorkspaceCore, case: &str, collection: &str, record: &str, a: &Value) -> CoreResponse {
    let to_version = a["to_version"].as_u64().unwrap();
    let restored_logical_at = a["restored_logical_at"].clone();

    // Capture the feed BEFORE the restore (non-destructive proof through the command).
    let feed_before = entries(&history(core, owner(), collection, record));

    let mut payload = json!({
        "collection": collection,
        "id": record,
        "to_version": to_version,
    });
    if !restored_logical_at.is_null() {
        payload["restored_logical_at"] = restored_logical_at;
    }
    let resp = core.handle(cmd(owner(), "db.restore", payload));
    assert!(resp.ok, "{case}: db.restore should succeed: {:?}", resp.error);
    assert_eq!(
        resp.payload["new_version"].as_u64().unwrap(),
        a["expect_new_version"].as_u64().unwrap(),
        "{case}: new version"
    );

    // Current state after restore (from the restore response + a fresh db.history).
    if a.get("expect_current_absent").and_then(Value::as_bool) == Some(true) {
        assert!(resp.payload["state"].is_null(), "{case}: expected current record absent");
    } else {
        assert_state_fields(case, &resp.payload["state"], &a["expect_current_fields"]);
    }

    // NON-DESTRUCTIVE: re-read the feed; earlier entries are byte-identical, and the
    // feed only grew.
    let feed_after = entries(&history(core, owner(), collection, record));
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

    // Prior versions still reconstructable through db.history's per-version state.
    if let Some(intact) = a.get("expect_prior_intact").and_then(Value::as_array) {
        for point in intact {
            let v = point["version"].as_u64().unwrap();
            let e = feed_after
                .iter()
                .find(|e| e["version"].as_u64().unwrap() == v)
                .unwrap_or_else(|| panic!("{case}: prior version {v} missing from feed"));
            assert_state_fields(case, &e["state"], &point["state_fields"]);
        }
    }
    resp
}

/// `restore` kind: the full non-destructive restore contract through the command.
fn run_restore(case: &str, fx: &Value) {
    let (mut core, collection, record) = seed(fx);
    let a = &fx["assert"];
    drive_restore(&mut core, case, &collection, &record, a);

    // Optional: a DL-6 rebuild reproduces the restored state (the restore op is in the
    // CRDT source of truth, not a side write).
    if let Some(rebuilt_fields) = a.get("rebuild_then_expect_fields") {
        core.rebuild_projection().unwrap();
        let after = entries(&history(&mut core, owner(), &collection, &record));
        let last = after.last().expect("a feed entry after rebuild");
        assert_state_fields(case, &last["state"], rebuilt_fields);
    }
}

/// `deterministic` kind: two `db.history` reads are byte-equal (no wall clock in the
/// read path), and a `db.restore` produces the listed new version + current state,
/// reproduced across a DL-6 rebuild.
fn run_deterministic(case: &str, fx: &Value) {
    let (mut core, collection, record) = seed(fx);
    let a = &fx["assert"];

    let h1 = entries(&history(&mut core, owner(), &collection, &record));
    let h2 = entries(&history(&mut core, owner(), &collection, &record));
    assert_eq!(h1, h2, "{case}: db.history must be byte-deterministic");

    drive_restore(&mut core, case, &collection, &record, a);

    // Rebuild from chunks and confirm the same current state (replay-identical).
    core.rebuild_projection().unwrap();
    let after = entries(&history(&mut core, owner(), &collection, &record));
    let last = after.last().expect("a feed entry after rebuild");
    assert_state_fields(case, &last["state"], &a["expect_current_fields"]);
}

/// `retention` kind: compacting with a `RetentionPolicy` keeps the within-window
/// change-feed entries (visible through `db.history`) and prunes beyond. Driven
/// through the workspace facade's `compact_history`, then re-read via `db.history`.
fn run_retention(case: &str, fx: &Value) {
    let (mut core, collection, record) = seed(fx);
    let a = &fx["assert"];
    let window = a["window"].as_u64().unwrap();
    assert!(
        a["all_peers_acked"].as_bool().unwrap_or(true),
        "{case}: harness models only the all-peers-acked path"
    );

    let opts = CompactionOptions::all_peers_acked().with_retention(RetentionPolicy::new(window));
    core.compact_history(&opts).unwrap();

    let feed = entries(&history(&mut core, owner(), &collection, &record));
    let retained: std::collections::BTreeSet<u64> =
        feed.iter().map(|e| e["version"].as_u64().unwrap()).collect();
    for v in a["expect_retained_versions"].as_array().unwrap() {
        let v = v.as_u64().unwrap();
        assert!(
            retained.contains(&v),
            "{case}: within-window change-feed entry v{v} must be retained, feed = {retained:?}"
        );
    }
    for v in a["expect_pruned_versions"].as_array().unwrap() {
        let v = v.as_u64().unwrap();
        assert!(
            !retained.contains(&v),
            "{case}: beyond-window change-feed entry v{v} may be pruned, feed = {retained:?}"
        );
    }
    // Review 166: a RETAINED within-window entry must keep its WHO/WHEN/WHAT — its
    // `state` (title) and `logical_at` — after compaction folds the older suffix into a
    // compact base. A naive `created_at`-prefix replay would report `state=None` /
    // `logical_at=None` for these LIVE retained changes (the compact base lands last in
    // write order); the frontier-ordered reconstruction keeps them intact, so the DL-20
    // retained 90-day undo/audit feed survives compaction through the live `db.history`.
    if let Some(expected_states) = a.get("expect_retained_state").and_then(Value::as_array) {
        for exp in expected_states {
            let v = exp["version"].as_u64().unwrap();
            let entry = feed
                .iter()
                .find(|e| e["version"].as_u64().unwrap() == v)
                .unwrap_or_else(|| panic!("{case}: retained entry v{v} missing from feed"));
            assert_state_fields(case, &entry["state"], &exp["state_fields"]);
            assert_eq!(
                entry["logical_at"], exp["logical_at"],
                "{case}: retained entry v{v} must keep its logical_at after compaction"
            );
        }
    }
    // Projection unchanged by compaction (DL-19 invariant): the live record is intact.
    let resp = core.handle(cmd(
        owner(),
        "query.execute",
        json!({ "collection": collection }),
    ));
    assert!(resp.ok, "{case}: query.execute should succeed: {:?}", resp.error);
    let row = resp.payload["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(record))
        .unwrap_or_else(|| panic!("{case}: the live record survives compaction"));
    assert_state_fields(case, &json!({ "fields": row["fields"] }), &a["expect_current_fields"]);
}

#[test]
fn dl20_time_travel_command_fixtures() {
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
            "history" => run_history(name, &fx),
            "state_at" => run_state_at(name, &fx),
            "restore" => run_restore(name, &fx),
            "deterministic" => run_deterministic(name, &fx),
            "retention" => run_retention(name, &fx),
            other => panic!("{name}: unknown assert kind {other}"),
        }
        ran += 1;
    }
    assert_eq!(ran, declared, "every time-travel fixture must run through the command path");
}

// --------------------------------------------------------------- live-wire proofs

/// LIVE: a `db.restore` driven through the real command path appends a NEW version
/// equal to a prior state, and the prior history stays intact — the non-destructive
/// keystone, proven end-to-end through `WorkspaceCore::handle` (not the storage method).
#[test]
fn live_db_restore_appends_a_new_version_and_keeps_prior_history() {
    let mut core = WorkspaceCore::in_memory("ws-live-restore").unwrap();
    let idx = IndexManager::new();
    for (n, body) in [(1, "A"), (2, "B"), (3, "C")] {
        apply_seed(
            &mut core,
            &idx,
            "tasks",
            &json!({ "op": if n == 1 { "insert" } else { "patch" }, "id": "t1", "fields": { "title": body }, "logical_at": n }),
        );
    }

    // The feed has three versions before the restore.
    let before = entries(&history(&mut core, owner(), "tasks", "t1"));
    assert_eq!(before.len(), 3, "three seeded versions: {before:?}");

    // Restore to version 1 (title "A") through the live command, pinning the logical
    // clock so the result is deterministic.
    let resp = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1, "restored_logical_at": 4 }),
    ));
    assert!(resp.ok, "db.restore should succeed: {:?}", resp.error);
    assert_eq!(resp.payload["new_version"], json!(4), "restore appends a new version");
    assert_eq!(resp.payload["state"]["fields"]["title"], json!("A"), "current state == v1");

    // The feed GREW by one and the prior three entries are byte-identical (history is
    // never rewritten); the new entry restores the v1 state.
    let after = entries(&history(&mut core, owner(), "tasks", "t1"));
    assert_eq!(after.len(), 4, "history grew by exactly one");
    assert_eq!(&after[..3], &before[..], "prior history is byte-identical after restore");
    assert_eq!(after[3]["version"], json!(4));
    assert_eq!(after[3]["state"]["fields"]["title"], json!("A"));

    // The intermediate versions are STILL in history (B@v2, C@v3) — nothing was
    // destroyed by the restore.
    assert_eq!(after[1]["state"]["fields"]["title"], json!("B"));
    assert_eq!(after[2]["state"]["fields"]["title"], json!("C"));
}

/// LIVE: a `db.restore` WITHOUT a pinned `restored_logical_at` still appends a
/// deterministic new version, drawing the WHEN from the workspace's monotone logical
/// clock (no wall clock) — and a DL-6 rebuild reproduces the restored state.
#[test]
fn live_db_restore_without_pinned_clock_is_replay_safe() {
    let mut core = WorkspaceCore::in_memory("ws-live-restore-clock").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "patch", "id": "t1", "fields": { "title": "B" }, "logical_at": 2 }));

    let resp = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1 }),
    ));
    assert!(resp.ok, "db.restore should succeed without a pinned clock: {:?}", resp.error);
    assert_eq!(resp.payload["new_version"], json!(3), "restore appends chunk-0003");
    assert_eq!(resp.payload["state"]["fields"]["title"], json!("A"));
    // The WHEN was drawn from a MONOTONE logical clock derived from the record's own
    // data frontier (review 167 P2): the seeded versions stamped logical_at 1 and 2, so
    // the omitted-clock default must be STRICTLY GREATER than the prior record timestamp
    // (2) — not merely non-null, and never colliding with / preceding a seeded version.
    // (A naive EventSink counter would start at 0 and could stamp 1, colliding with the
    // original insert and PRECEDING the change it undid.)
    let stamped = resp.payload["restored_logical_at"]
        .as_i64()
        .unwrap_or_else(|| panic!("the restore stamps a logical timestamp: {}", resp.payload));
    assert!(
        stamped > 2,
        "the omitted-clock restore must stamp a logical_at strictly greater than the prior \
         record timestamp (2), got {stamped}"
    );

    // Replay-safe: a DL-6 rebuild from the CRDT source of truth reproduces the state.
    core.rebuild_projection().unwrap();
    let feed = entries(&history(&mut core, owner(), "tasks", "t1"));
    assert_eq!(feed.last().unwrap()["state"]["fields"]["title"], json!("A"));
}

/// LIVE / RBAC: a `db.history` call by an actor whose TRUSTED `db.read` scope does NOT
/// include the collection is rejected — the read scope is read from the workspace's
/// trusted grant table, NEVER the request payload (review 048/050).
#[test]
fn live_db_history_denied_when_db_read_scope_excludes_the_collection() {
    let mut core = WorkspaceCore::in_memory("ws-live-history-deny").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));

    // Provision a NARROWED trusted db.read scope for "dev": only "notes", not "tasks".
    core.grant_db_read("dev", ["notes"]).unwrap();

    // A history read of "tasks" is denied (the trusted scope excludes it) — even though
    // the owner role would otherwise read, and even if the payload smuggled a wider
    // grant (which is ignored / rejected as self-escalation).
    let denied = history(&mut core, owner(), "tasks", "t1");
    assert!(!denied.ok, "db.history outside the trusted db.read scope must be denied");
    assert_eq!(
        denied.error.as_ref().unwrap().code(),
        "CapabilityRequired",
        "the collection is outside the granted db.read scope"
    );

    // A payload that smuggles a wider db.read grant cannot widen the trusted scope: it
    // is rejected as a self-escalation, not honored.
    let smuggled = core.handle(cmd(
        owner(),
        "db.history",
        json!({ "collection": "tasks", "id": "t1", "grants": { "db": { "read": ["tasks"] } } }),
    ));
    assert!(!smuggled.ok, "a payload db.read grant cannot widen the trusted scope");
    assert_eq!(smuggled.error.as_ref().unwrap().code(), "PermissionDenied");

    // The SAME read, of a collection WITHIN the trusted scope, is allowed.
    let allowed = history(&mut core, owner(), "notes", "n1");
    assert!(allowed.ok, "a read within the trusted scope succeeds: {:?}", allowed.error);
}

/// LIVE / RBAC: a read-only role (Viewer) can read history but CANNOT restore — a
/// restore is a record WRITE, gated by `db.write` (the role gate denies the Viewer
/// before any version is appended).
#[test]
fn live_db_restore_denied_for_a_read_only_role() {
    let mut core = WorkspaceCore::in_memory("ws-live-restore-deny").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "patch", "id": "t1", "fields": { "title": "B" }, "logical_at": 2 }));

    let viewer = ActorContext { actor: "viewer-1".into(), role: Role::Viewer };

    // A Viewer CAN read the change feed (db.read role).
    let read = history(&mut core, viewer.clone(), "tasks", "t1");
    assert!(read.ok, "a Viewer may read history: {:?}", read.error);

    // But a Viewer CANNOT restore (db.write role gate denies before any append).
    let restore = core.handle(cmd(
        viewer,
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1 }),
    ));
    assert!(!restore.ok, "a Viewer cannot db.restore (it is a record write)");
    assert_eq!(restore.error.as_ref().unwrap().code(), "PermissionDenied");

    // The denial left the history untouched (no new version appended).
    let after = entries(&history(&mut core, owner(), "tasks", "t1"));
    assert_eq!(after.len(), 2, "a denied restore appends nothing: {after:?}");
}

/// LIVE / RBAC: an Editor with a NARROWED trusted `db.write` scope is denied a restore
/// of a collection outside that scope (the write scope is trusted, never the payload).
#[test]
fn live_db_restore_denied_outside_trusted_db_write_scope() {
    let mut core = WorkspaceCore::in_memory("ws-live-restore-scope").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "patch", "id": "t1", "fields": { "title": "B" }, "logical_at": 2 }));

    let editor = ActorContext { actor: "editor-1".into(), role: Role::Editor };
    // Trust the editor to write ONLY "notes", not "tasks".
    core.grant_db_write("editor-1", ["notes"]).unwrap();

    let denied = core.handle(cmd(
        editor.clone(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1 }),
    ));
    assert!(!denied.ok, "restore outside the trusted db.write scope is denied");
    assert_eq!(
        denied.error.as_ref().unwrap().code(),
        "CapabilityRequired",
        "the collection is outside the granted db.write scope"
    );

    // A payload smuggling a wider db.write grant is rejected, not honored.
    let smuggled = core.handle(cmd(
        editor,
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1, "grants": { "db": { "write": ["tasks"] } } }),
    ));
    assert!(!smuggled.ok, "a payload db.write grant cannot widen the trusted scope");
    assert_eq!(smuggled.error.as_ref().unwrap().code(), "PermissionDenied");

    // The history is untouched by the denied restores.
    let after = entries(&history(&mut core, owner(), "tasks", "t1"));
    assert_eq!(after.len(), 2, "denied restores append nothing");
}

/// `db.history` / `db.restore` validate their required payload fields (a malformed
/// command is a `ValidationError`, never a silent default).
#[test]
fn time_travel_commands_validate_their_payload() {
    let mut core = WorkspaceCore::in_memory("ws-tt-validate").unwrap();

    // Missing `id`.
    let no_id = core.handle(cmd(owner(), "db.history", json!({ "collection": "tasks" })));
    assert!(!no_id.ok);
    assert_eq!(no_id.error.as_ref().unwrap().code(), "ValidationError");

    // Missing `to_version`.
    let no_version = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1" }),
    ));
    assert!(!no_version.ok);
    assert_eq!(no_version.error.as_ref().unwrap().code(), "ValidationError");

    // A non-integer `restored_logical_at` is rejected, not coerced.
    let bad_clock = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1, "restored_logical_at": "soon" }),
    ));
    assert!(!bad_clock.ok);
    assert_eq!(bad_clock.error.as_ref().unwrap().code(), "ValidationError");
}

// --------------------------------------------------- review 167 regression proofs

/// The canonical `db.watch.notification` event payloads emitted so far (the observable
/// notification stream the spine delivered), mirroring `live_query_spine.rs`.
fn notifications(core: &WorkspaceCore) -> Vec<Value> {
    core.events()
        .events_of_kind("db.watch.notification")
        .map(|e| e.payload.clone())
        .collect()
}

/// LIVE (review 167 P1): a `db.watch` registered over the collection receives a real
/// notification turn AFTER a `db.restore` — proving the restore routes through the SAME
/// DL-16 committed-mutation notification path as a `ctx.db` write (it did NOT before
/// this fix: a restore is a new insert/delete that dirties the target id, but the
/// restore command bypassed the watch loop, so an active watch stayed STALE). The watch
/// is registered through the trusted in-process seam (a substrate-only watch: no applet
/// callback to re-enter, but the notification is still computed/emitted/recorded), and
/// the restore is driven through the live command path — not a faked `commit_and_notify`.
#[test]
fn live_db_restore_notifies_an_active_watch_over_the_collection() {
    let mut core = WorkspaceCore::in_memory("ws-restore-watch").unwrap();
    let idx = IndexManager::new();
    // Seed three versions of t1 through the real DL-4 CRDT path.
    for (n, body) in [(1, "A"), (2, "B"), (3, "C")] {
        apply_seed(
            &mut core,
            &idx,
            "tasks",
            &json!({ "op": if n == 1 { "insert" } else { "patch" }, "id": "t1", "fields": { "title": body }, "logical_at": n }),
        );
    }

    // Register a live watch over the WHOLE collection (the trusted in-process seam —
    // no installed applet is needed for a substrate watch's notification to fire).
    core.register_watch("watcher", "watch:tasks", "onWatch", json!({ "from": "tasks" }))
        .unwrap();
    assert_eq!(core.active_watch_ids(), vec!["watch:tasks".to_string()]);
    assert!(
        notifications(&core).is_empty(),
        "no mutation since the watch registered → no notification yet"
    );

    // Restore t1 to version 1 (title "A") through the live command path.
    let resp = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1, "restored_logical_at": 4 }),
    ));
    assert!(resp.ok, "db.restore should succeed: {:?}", resp.error);

    // PROOF the watch FIRED on the restore: exactly one notification, naming the watch +
    // the restored record id + the post-restore result (the restore is a new insert).
    let notes = notifications(&core);
    assert_eq!(notes.len(), 1, "the restore fired exactly one notification: {notes:?}");
    let n = &notes[0];
    assert_eq!(n["watch_id"], json!("watch:tasks"));
    assert_eq!(n["record_ids"], json!(["t1"]));
    assert_eq!(n["result_ids"], json!(["t1"]));
    assert_eq!(n["reason"], json!("insert"), "a restore-to-live-state is a re-insert");
}

/// LIVE (review 167 P2): a `db.restore` with an OMITTED `restored_logical_at` stamps a
/// MONOTONE default — strictly greater than the prior record timestamp (and thus greater
/// than the change it undid) — derived from the record's own data frontier, NOT the
/// EventSink event counter (which starts independently at 0 and would collide with /
/// precede the seeded timestamps). The seeds stamp logical_at 1 and 2, so the default
/// must be > 2.
#[test]
fn live_db_restore_omitted_clock_is_strictly_greater_than_prior_timestamp() {
    let mut core = WorkspaceCore::in_memory("ws-restore-monotone").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "patch", "id": "t1", "fields": { "title": "B" }, "logical_at": 2 }));

    // The record's current data frontier (max logical_at across its history) is 2.
    let feed_before = entries(&history(&mut core, owner(), "tasks", "t1"));
    let prior_frontier = feed_before
        .iter()
        .filter_map(|e| e["logical_at"].as_i64())
        .max()
        .unwrap();
    assert_eq!(prior_frontier, 2, "the seeded frontier is 2");

    // Restore to version 1 WITHOUT pinning the clock.
    let resp = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1 }),
    ));
    assert!(resp.ok, "db.restore should succeed without a pinned clock: {:?}", resp.error);

    // The default stamp is STRICTLY GREATER than the prior record timestamp (never a
    // collision with / a value preceding a seeded version).
    let stamped = resp.payload["restored_logical_at"].as_i64().unwrap();
    assert!(
        stamped > prior_frontier,
        "the omitted-clock restore must stamp a logical_at strictly greater than the prior \
         record timestamp ({prior_frontier}), got {stamped}"
    );

    // The new restore entry carries that monotone WHEN in the change feed.
    let feed_after = entries(&history(&mut core, owner(), "tasks", "t1"));
    let restored = feed_after.last().unwrap();
    assert_eq!(
        restored["logical_at"].as_i64().unwrap(),
        stamped,
        "the restore entry reports the monotone default logical_at"
    );
    assert!(
        restored["logical_at"].as_i64().unwrap() > prior_frontier,
        "the restore entry's logical_at exceeds every prior version's"
    );
}

/// LIVE (review 169): the monotone default `restored_logical_at` must account for a
/// DELETE's timestamp. `insert@1 -> patch@2 -> delete@100`, then restore-to-v1 with an
/// OMITTED clock: the data frontier's max non-delete logical_at is 2, but the delete
/// happened at 100 — so the default must be STRICTLY GREATER than 100 (the change it
/// undid), not `max(non-delete) + 1 == 3`. A delete leaves no surviving envelope, so its
/// WHEN is recovered from the delete's own mutation timestamp (carried on the oplog
/// row) and surfaced on the change feed, which is what lets the monotone clock see it.
#[test]
fn live_db_restore_omitted_clock_is_strictly_greater_than_a_late_delete() {
    let mut core = WorkspaceCore::in_memory("ws-restore-monotone-delete").unwrap();
    let idx = IndexManager::new();
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "insert", "id": "t1", "fields": { "title": "A" }, "logical_at": 1 }));
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "patch", "id": "t1", "fields": { "title": "B" }, "logical_at": 2 }));
    // A LATE delete: its logical_at (100) jumps well past the live-state frontier (2).
    apply_seed(&mut core, &idx, "tasks", &json!({ "op": "delete", "id": "t1", "logical_at": 100 }));
    assert!(core.store_mut().get_record("tasks", "t1").unwrap().is_none(), "deleted");

    // The change feed must surface the delete's WHEN (100), so the monotone frontier is
    // the delete timestamp, not just the max live-state version (2).
    let feed_before = entries(&history(&mut core, owner(), "tasks", "t1"));
    let delete_entry = feed_before
        .iter()
        .find(|e| e["kind"] == json!("record.delete"))
        .expect("delete in the change feed");
    assert_eq!(
        delete_entry["logical_at"].as_i64(),
        Some(100),
        "the delete version reports its own logical_at (review 169)"
    );

    // Restore to version 1 (a live state, over the tombstone) WITHOUT pinning the clock.
    let resp = core.handle(cmd(
        owner(),
        "db.restore",
        json!({ "collection": "tasks", "id": "t1", "to_version": 1 }),
    ));
    assert!(resp.ok, "db.restore should succeed without a pinned clock: {:?}", resp.error);

    // The default stamp is STRICTLY GREATER than the delete it undid (100) — NOT 3
    // (max non-delete logical_at + 1), which would precede the delete.
    let stamped = resp.payload["restored_logical_at"].as_i64().unwrap();
    assert!(
        stamped > 100,
        "the omitted-clock restore must stamp a logical_at strictly greater than the delete \
         it undid (100), got {stamped}"
    );

    // The record came back (restore-to-v1 reinserts over the tombstone), and the restore
    // entry carries that monotone WHEN — still after the delete — in the change feed.
    let now = core.store_mut().get_record("tasks", "t1").unwrap().unwrap();
    assert_eq!(now.fields["title"], json!("A"), "the record is restored to v1");
    let feed_after = entries(&history(&mut core, owner(), "tasks", "t1"));
    let restored = feed_after.last().unwrap();
    assert_eq!(restored["logical_at"].as_i64().unwrap(), stamped);
    assert!(
        restored["logical_at"].as_i64().unwrap() > 100,
        "the restore entry's logical_at exceeds the delete it reversed"
    );
}
