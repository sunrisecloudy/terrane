//! SS-7 end-to-end: the apply-time authorization gate wired into
//! [`WorkspaceCore::sync_with`] (`forge/spec/sync-rbac.md`). Phase-1
//! (`authorize_remote_op`) is enforced BEFORE any CRDT chunk is imported, so an
//! unauthorized incoming op is rejected and never touches the receiver's history
//! or projection.
//!
//! Two whole cores are built with a seeded receiver-side membership table:
//!
//! 1. `authorized_editor_op_applies_and_cores_converge` — an editor with
//!    `db.write` on the collection: the op applies and both cores converge
//!    byte-identically (same projection, same chunk history).
//! 2. `unauthorized_viewer_op_is_rejected_before_import` — a viewer (no write
//!    role): the op is REJECTED — the receiver imports nothing (projection
//!    unchanged, no new record), a `permission_denied` is surfaced, and an audit
//!    denial naming the actor/collection/role is recorded.
//! 3. `editor_outside_db_write_scope_is_rejected` — an editor whose trusted
//!    `db.write` does NOT cover the collection: rejected the same way (the role
//!    matrix passes but the collection grant denies).

use forge_core::{source_id_for, Capability, RunPolicy, TrustedMembership, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use forge_storage::{IndexManager, Mutation};
use serde_json::{json, Value};

// Distinct Loro peer ids so concurrent edits would converge to one agreed winner.
const SENDER_PEER: u64 = 700;
const RECEIVER_PEER: u64 = 800;

fn membership(actor: &str, role: Role, db_write: &[&str]) -> TrustedMembership {
    membership_full(actor, role, db_write, false)
}

/// A membership row with an explicit `schema_write` grant. A DL-13 migration chunk is
/// a schema-affecting op (review 143), so a peer that may apply one needs BOTH db.write
/// on the collection AND schema-change authority (Owner/Maintainer + `schema_write`).
fn membership_full(
    actor: &str,
    role: Role,
    db_write: &[&str],
    schema_write: bool,
) -> TrustedMembership {
    TrustedMembership {
        actor_id: actor.into(),
        role,
        db_read: vec!["*".into()],
        db_write: db_write.iter().map(|s| s.to_string()).collect(),
        schema_write,
    }
}

fn insert(id: &str, fields: Value, at: i64) -> Mutation {
    Mutation::Insert {
        collection: "tasks".into(),
        id: Some(id.into()),
        fields: fields.as_object().unwrap().clone(),
        logical_at: Some(at),
    }
}

/// Read a core's `tasks` projection back through the PUBLIC `query.execute`
/// command as a sorted `(id, fields)` list.
fn query_tasks(core: &mut WorkspaceCore) -> Vec<(String, Value)> {
    let cmd = CoreCommand {
        request_id: RequestId::new("req"),
        name: "query.execute".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("dev"),
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({ "collection": "tasks" }),
    };
    let resp = core.handle(cmd);
    assert!(resp.ok, "query.execute failed: {:?}", resp.error);
    let mut rows: Vec<(String, Value)> = resp.payload["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .map(|r| (r["id"].as_str().unwrap().to_string(), r["fields"].clone()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// Build a (sender, receiver) pair, both empty, with distinct Loro peer ids. The
/// receiver seeds `membership` for the sender's source id; how the sender is
/// trusted on the receiver decides whether its op applies.
fn cores_with_membership(receiver_trusts: TrustedMembership) -> (WorkspaceCore, WorkspaceCore) {
    let mut sender = WorkspaceCore::in_memory("ws-sender").unwrap();
    let mut receiver = WorkspaceCore::in_memory("ws-receiver").unwrap();
    sender.store_mut().set_crdt_peer_id(SENDER_PEER);
    receiver.store_mut().set_crdt_peer_id(RECEIVER_PEER);
    // The receiver's trusted row for the sender's authenticated session.
    receiver
        .set_peer_membership(source_id_for(SENDER_PEER), receiver_trusts)
        .unwrap();
    // The sender trusts the (empty) receiver as owner so the symmetric back-channel
    // never spuriously denies — only the sender→receiver direction is under test.
    sender
        .set_peer_membership(
            source_id_for(RECEIVER_PEER),
            membership("actor-receiver", Role::Owner, &["*"]),
        )
        .unwrap();
    (sender, receiver)
}

#[test]
fn authorized_editor_op_applies_and_cores_converge() {
    let idx = IndexManager::new();
    // The receiver trusts the sender as an editor WITH db.write on `tasks`.
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));

    // The sender authors a record locally (a real CRDT op + oplog row).
    sender
        .store_mut()
        .apply_mutation_crdt(
            &insert("task-1", json!({ "title": "authorized", "status": "open" }), 1),
            &idx,
        )
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert!(report.total_chunks_moved() > 0, "the authorized op moves a chunk");
    assert_eq!(report.chunks_denied, 0, "no op should be denied");

    // The receiver imported the op: the record is present and the two cores agree.
    let recv_rows = query_tasks(&mut receiver);
    let send_rows = query_tasks(&mut sender);
    assert_eq!(recv_rows, send_rows, "cores must converge after an authorized op");
    assert_eq!(recv_rows.len(), 1);
    assert_eq!(recv_rows[0].0, "task-1");
    assert_eq!(recv_rows[0].1["title"], json!("authorized"));

    // The chunk histories are byte-identical (true convergence, not just the
    // projection): same doc, same chunk set.
    let doc = forge_storage::collection_doc_id("tasks");
    let recv_chunks: Vec<Vec<u8>> = receiver
        .store()
        .get_chunks(&doc)
        .unwrap()
        .into_iter()
        .map(|c| c.payload)
        .collect();
    let send_chunks: Vec<Vec<u8>> = sender
        .store()
        .get_chunks(&doc)
        .unwrap()
        .into_iter()
        .map(|c| c.payload)
        .collect();
    assert_eq!(recv_chunks, send_chunks, "chunk histories converge byte-identically");

    // An allow audit row was recorded on the receiver.
    let allowed = receiver
        .events()
        .events_of_kind("sync.authorized")
        .count();
    assert_eq!(allowed, 1, "exactly one authorized op audited");
}

#[test]
fn unauthorized_viewer_op_is_rejected_before_import() {
    let idx = IndexManager::new();
    // The receiver trusts the sender only as a VIEWER (no write role).
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-viewer", Role::Viewer, &[]));

    sender
        .store_mut()
        .apply_mutation_crdt(
            &insert("task-x", json!({ "title": "viewer should not write" }), 1),
            &idx,
        )
        .unwrap();

    // Snapshot the receiver's projection BEFORE sync.
    let before = query_tasks(&mut receiver);
    assert!(before.is_empty(), "receiver starts empty");

    let report = sender.sync_with(&mut receiver).unwrap();

    // The op was REJECTED: a chunk was denied, none imported into the receiver.
    assert_eq!(report.chunks_denied, 1, "the viewer write must be denied");
    assert_eq!(report.chunks_a_to_b, 0, "no chunk imported into the receiver");

    // The receiver's projection is UNCHANGED — no new record.
    let after = query_tasks(&mut receiver);
    assert_eq!(after, before, "the rejected op left the projection unchanged");
    assert!(after.is_empty(), "no record was imported");

    // The receiver imported NO CRDT chunk for the collection.
    let doc = forge_storage::collection_doc_id("tasks");
    assert!(
        receiver.store().get_chunks(&doc).unwrap().is_empty(),
        "no chunk landed in the receiver's history"
    );

    // A permission_denied audit denial was recorded, naming the trusted role +
    // collection and the role-based reason.
    let denials: Vec<&forge_domain::CoreEvent> = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .collect();
    assert_eq!(denials.len(), 1, "exactly one denial audited");
    let payload = &denials[0].payload;
    assert_eq!(payload["decision"], json!("deny"));
    assert_eq!(payload["collection"], json!("tasks"));
    assert_eq!(payload["actor_id"], json!("actor-viewer"));
    assert!(
        payload["reason"].as_str().unwrap().contains("viewer"),
        "denial reason names the viewer role: {payload:?}"
    );
}

#[test]
fn editor_outside_db_write_scope_is_rejected() {
    let idx = IndexManager::new();
    // Editor role passes the matrix, but trusted db.write covers only `notes`.
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["notes"]));

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-y", json!({ "title": "out of scope" }), 1), &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 1, "out-of-scope write denied");
    assert_eq!(report.chunks_a_to_b, 0, "nothing imported");

    assert!(query_tasks(&mut receiver).is_empty(), "projection unchanged");
    let doc = forge_storage::collection_doc_id("tasks");
    assert!(receiver.store().get_chunks(&doc).unwrap().is_empty());

    let denial = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .next()
        .expect("a denial was audited");
    assert!(
        denial.payload["reason"]
            .as_str()
            .unwrap()
            .contains("does not include tasks"),
        "denial names the missing collection grant: {:?}",
        denial.payload
    );
}

// --- SC-10 workspace-policy gate at the SS-7 sync boundary (T037 / review 164) ---
//
// SC-10 is evaluated on EVERY command AND EVERY remote sync op. The receiver's
// trusted `RunPolicy` therefore gates an incoming op the same way it gates a local
// `ctx.*` call: a remote record write is a `Db`-category op, so a receiver that
// forbids the `db` category SKIPS the chunk even when its `sync_membership` would
// allow it. These tests pin the wiring in `authorize_incoming_op`.

#[test]
fn workspace_policy_db_deny_skips_otherwise_rbac_allowed_remote_op() {
    let idx = IndexManager::new();
    // RBAC would ALLOW: the receiver trusts the sender as an editor WITH db.write on
    // `tasks`. The only thing that can deny is the SC-10 workspace-policy gate.
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));

    // The receiver's trusted workspace admin policy forbids the `db` capability
    // category (set ONLY through the trusted seam, never the incoming op payload).
    receiver
        .set_run_policy(RunPolicy {
            workspace_denied: vec![Capability::Db],
            ..RunPolicy::default()
        })
        .unwrap();

    sender
        .store_mut()
        .apply_mutation_crdt(
            &insert("task-1", json!({ "title": "rbac-allowed but policy-denied" }), 1),
            &idx,
        )
        .unwrap();

    let before = query_tasks(&mut receiver);
    assert!(before.is_empty(), "receiver starts empty");

    let report = sender.sync_with(&mut receiver).unwrap();

    // The op is SKIPPED by the workspace-policy gate even though membership allowed
    // it: a chunk is denied and NONE imported into the receiver.
    assert_eq!(
        report.chunks_denied, 1,
        "a workspace-policy db deny must skip the otherwise-RBAC-allowed op"
    );
    assert_eq!(report.chunks_a_to_b, 0, "no chunk imported into the receiver");

    // Projection + chunk history unchanged.
    assert_eq!(query_tasks(&mut receiver), before, "projection unchanged");
    let doc = forge_storage::collection_doc_id("tasks");
    assert!(
        receiver.store().get_chunks(&doc).unwrap().is_empty(),
        "no chunk landed in the receiver's history"
    );

    // A permission_denied audit denial naming the workspace-policy gate was recorded.
    let denial = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .next()
        .expect("a denial was audited");
    assert_eq!(denial.payload["decision"], json!("deny"));
    assert_eq!(denial.payload["collection"], json!("tasks"));
    assert!(
        denial.payload["reason"]
            .as_str()
            .unwrap()
            .contains("workspace policy"),
        "denial reason names the workspace-policy gate: {:?}",
        denial.payload
    );
    // The durable audit row tags the deciding gate so the skip is attributable.
    let rows = receiver
        .store()
        .query_audit(&forge_storage::AuditQuery::by_decision("deny"))
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.metadata["gate"] == json!("workspace-policy")),
        "a durable workspace-policy deny audit row was persisted in the receiver's log: {rows:?}"
    );
}

#[test]
fn workspace_policy_gate_first_failing_beats_membership_rbac() {
    // The same incoming op fails BOTH the workspace-policy gate (db denied) AND
    // membership RBAC (viewer cannot write). SC-10 order: workspace-policy (gate 2)
    // is evaluated BEFORE the role/grant checks, so the surfaced reason names the
    // workspace-policy gate, not the viewer role. First-failing-gate wins.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-viewer", Role::Viewer, &[]));
    receiver
        .set_run_policy(RunPolicy {
            workspace_denied: vec![Capability::Db],
            ..RunPolicy::default()
        })
        .unwrap();

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-2", json!({ "title": "doubly denied" }), 1), &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 1);
    assert_eq!(report.chunks_a_to_b, 0);

    let denial = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .next()
        .expect("a denial was audited");
    let reason = denial.payload["reason"].as_str().unwrap();
    assert!(
        reason.contains("workspace policy"),
        "workspace-policy gate runs first, so it names the surfaced reason: {reason}"
    );
    assert!(
        !reason.contains("viewer"),
        "the later membership role denial is not the surfaced reason: {reason}"
    );
}

#[test]
fn unprovisioned_run_policy_does_not_block_rbac_allowed_remote_op() {
    // Baseline / no-regression: with NO RunPolicy set on the receiver, an
    // RBAC-allowed op imports exactly as before (the SS-7 SC-10 wiring is opt-in and
    // default-open — shells tighten, never loosen).
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));
    assert!(receiver.run_policy().is_none(), "no policy configured");

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-3", json!({ "title": "allowed" }), 1), &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 0, "no SC-10 deny when unprovisioned");
    assert!(report.total_chunks_moved() > 0, "the op moved a chunk");
    let rows = query_tasks(&mut receiver);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "task-3");
}

#[test]
fn workspace_policy_allowing_db_still_imports_remote_op() {
    // A RunPolicy that explicitly ALLOWS `db` (and leaves the other gates
    // unspecified → all-categories) must NOT block an RBAC-allowed op: the gate only
    // denies a category it forbids, proving the deny in the prior test was the db
    // forbid, not the mere presence of a policy.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));
    receiver
        .set_run_policy(RunPolicy {
            workspace_allowed: Some(vec![
                Capability::Db,
                Capability::Storage,
                Capability::Ui,
                Capability::Time,
                Capability::Random,
            ]),
            ..RunPolicy::default()
        })
        .unwrap();

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-4", json!({ "title": "db allowed" }), 1), &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 0, "db is allowed, so no SC-10 deny");
    let rows = query_tasks(&mut receiver);
    assert_eq!(rows.len(), 1, "the op imported under a db-allowing policy");
    assert_eq!(rows[0].0, "task-4");
}

#[test]
fn forwarded_chunk_is_authorized_against_original_author_not_relay() {
    // review 092 #1: a three-peer relay regression. C writes; A imports C's chunk
    // (A is only a relay); A then syncs to B. B trusts A as an owner but does NOT
    // trust C, so A->B must DENY C's forwarded chunk — the receiver authorizes the
    // ORIGINAL author (C), not the relay (A). This is SS-7 actor identity: a peer A
    // is trusted cannot launder a write from an untrusted peer C through A.
    let idx = IndexManager::new();
    const C_PEER: u64 = 900;
    let mut c = WorkspaceCore::in_memory("ws-c").unwrap();
    let mut a = WorkspaceCore::in_memory("ws-a").unwrap();
    let mut b = WorkspaceCore::in_memory("ws-b").unwrap();
    c.store_mut().set_crdt_peer_id(C_PEER);
    a.store_mut().set_crdt_peer_id(SENDER_PEER);
    b.store_mut().set_crdt_peer_id(RECEIVER_PEER);

    // A trusts C as an owner, so C->A applies (A becomes a relay holding C's chunk).
    a.set_peer_membership(
        source_id_for(C_PEER),
        membership("actor-c", Role::Owner, &["*"]),
    )
    .unwrap();
    // C trusts A as owner so the symmetric back-channel never spuriously denies.
    c.set_peer_membership(
        source_id_for(SENDER_PEER),
        membership("actor-a", Role::Owner, &["*"]),
    )
    .unwrap();
    // B trusts A as an OWNER but seeds NO row for C. A relay of C's chunk must still
    // be gated against C — and C is unknown to B — so it is denied.
    b.set_peer_membership(
        source_id_for(SENDER_PEER),
        membership("actor-a", Role::Owner, &["*"]),
    )
    .unwrap();
    // A trusts B as owner for the symmetric back-channel.
    a.set_peer_membership(
        source_id_for(RECEIVER_PEER),
        membership("actor-b", Role::Owner, &["*"]),
    )
    .unwrap();

    // C authors a record; A imports it as a relay.
    c.store_mut()
        .apply_mutation_crdt(&insert("task-c", json!({ "title": "from C" }), 1), &idx)
        .unwrap();
    let c_to_a = c.sync_with(&mut a).unwrap();
    assert_eq!(c_to_a.chunks_denied, 0, "A trusts C, so C's chunk applies on A");
    assert_eq!(query_tasks(&mut a).len(), 1, "A imported C's record");

    // A -> B: A only relays C's chunk. B trusts A but not C, so the forwarded chunk
    // is DENIED (authorized against C, who is unknown to B).
    let a_to_b = a.sync_with(&mut b).unwrap();
    assert_eq!(a_to_b.chunks_denied, 1, "C's forwarded chunk must be denied at B");
    assert!(
        query_tasks(&mut b).is_empty(),
        "B imported nothing — C's write was not laundered through relay A"
    );
    let doc = forge_storage::collection_doc_id("tasks");
    assert!(
        b.store().get_chunks(&doc).unwrap().is_empty(),
        "no forwarded chunk landed in B's history"
    );

    // B audited a permission_denied naming the missing trust for C's source.
    let denial = b
        .events()
        .events_of_kind("sync.permission_denied")
        .next()
        .expect("B audited a denial for the forwarded chunk");
    assert_eq!(denial.payload["decision"], json!("deny"));
    assert_eq!(
        denial.payload["source"],
        json!(source_id_for(C_PEER)),
        "the denial names C (the original author), not relay A"
    );
}

#[test]
fn forwarded_chunk_is_authorized_against_original_author_positive_twin() {
    // review 092 #1 (positive twin of the three-peer regression): C writes; A imports
    // C's chunk (A is a relay); A syncs to B. B trusts A as an owner AND trusts C as an
    // editor WITH db.write on `tasks`. The forwarded chunk is gated against C (the
    // ORIGINAL author) — who IS authorized here — so it APPLIES and B converges with C.
    let idx = IndexManager::new();
    const C_PEER: u64 = 901;
    let mut c = WorkspaceCore::in_memory("ws-c2").unwrap();
    let mut a = WorkspaceCore::in_memory("ws-a2").unwrap();
    let mut b = WorkspaceCore::in_memory("ws-b2").unwrap();
    c.store_mut().set_crdt_peer_id(C_PEER);
    a.store_mut().set_crdt_peer_id(SENDER_PEER);
    b.store_mut().set_crdt_peer_id(RECEIVER_PEER);

    // A trusts C; C trusts A (back-channel); A trusts B (back-channel).
    a.set_peer_membership(source_id_for(C_PEER), membership("actor-c", Role::Owner, &["*"]))
        .unwrap();
    c.set_peer_membership(
        source_id_for(SENDER_PEER),
        membership("actor-a", Role::Owner, &["*"]),
    )
    .unwrap();
    a.set_peer_membership(
        source_id_for(RECEIVER_PEER),
        membership("actor-b", Role::Owner, &["*"]),
    )
    .unwrap();
    // B trusts the relay A as an owner AND the original author C as an editor on `tasks`.
    b.set_peer_membership(
        source_id_for(SENDER_PEER),
        membership("actor-a", Role::Owner, &["*"]),
    )
    .unwrap();
    b.set_peer_membership(
        source_id_for(C_PEER),
        membership("actor-c", Role::Editor, &["tasks"]),
    )
    .unwrap();

    // C authors; A imports as a relay.
    c.store_mut()
        .apply_mutation_crdt(&insert("task-c", json!({ "title": "from C" }), 1), &idx)
        .unwrap();
    c.sync_with(&mut a).unwrap();
    assert_eq!(query_tasks(&mut a).len(), 1, "A imported C's record as a relay");

    // A -> B: A relays C's chunk; B authorizes it against C (trusted) -> applied.
    let a_to_b = a.sync_with(&mut b).unwrap();
    assert_eq!(a_to_b.chunks_denied, 0, "C is trusted by B, so the forwarded chunk applies");
    let b_rows = query_tasks(&mut b);
    assert_eq!(b_rows.len(), 1, "B imported C's forwarded record");
    assert_eq!(b_rows[0].0, "task-c");
    assert_eq!(b_rows[0].1["title"], json!("from C"));

    // The allow audit on B names C (the original author), not relay A.
    let allowed = b
        .events()
        .events_of_kind("sync.authorized")
        .next()
        .expect("B audited the authorized forwarded op");
    assert_eq!(
        allowed.payload["source"],
        json!(source_id_for(C_PEER)),
        "the allow names C (the original author), not relay A"
    );
}

#[test]
fn malformed_non_collection_doc_chunk_is_denied_before_import() {
    // review 092 #2: a chunk whose doc id is NOT a `collection/<name>` records doc
    // must be denied fail-closed at the apply boundary — the receiver must reject a
    // malformed chunk instead of guessing a collection / leaving the resource
    // unidentified. Here the sender holds an opaque chunk under a non-records doc id;
    // even with the sender trusted as an owner, the receiver denies it.
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-owner", Role::Owner, &["*"]));
    // Put a chunk under a non-records doc id directly on the sender's store.
    sender
        .store_mut()
        .put_chunk("applet/src", "chunk-0001", forge_sync::SYNC_CHUNK_FORMAT, b"opaque")
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 1, "the malformed-doc chunk must be denied");
    assert_eq!(report.chunks_a_to_b, 0, "nothing imported into the receiver");
    assert!(
        receiver.store().get_chunks("applet/src").unwrap().is_empty(),
        "the malformed chunk did not land in the receiver"
    );

    let denial = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .next()
        .expect("a denial was audited for the malformed chunk");
    assert_eq!(denial.payload["decision"], json!("deny"));
    assert!(
        denial.payload["reason"]
            .as_str()
            .unwrap()
            .contains("not a collection/<name>"),
        "the denial names the malformed doc id: {:?}",
        denial.payload
    );
}

#[test]
fn multi_record_transact_group_applies_and_converges() {
    // review 093 (positive regression): a MULTI-record transact group is ONE chunk
    // that legitimately names SEVERAL records. The wired envelope translation threads
    // the FULL touched-record list through `RemoteOpEnvelope.record_ids`, so the
    // envelope-metadata gate (which now rejects ONLY a truly empty/unknown list)
    // accepts it and the collection grant gates the op as a whole. Before the fix the
    // adapter collapsed any list that was not exactly one id to `record_id = None`,
    // and the gate denied a legitimate group as "missing record id", silently breaking
    // convergence. The group must apply (0 denied) and BOTH cores converge with all
    // records present. The trusted sender is an editor WITH db.write on `tasks`.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));

    // One transact group authoring two records into the same collection (one chunk).
    let group = vec![
        insert("task-1", json!({ "title": "first" }), 1),
        insert("task-2", json!({ "title": "second" }), 2),
    ];
    sender
        .store_mut()
        .transact_mutations_crdt(&group, &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(
        report.chunks_denied, 0,
        "a legitimate multi-record transact group must NOT be denied (it names concrete records)"
    );
    assert!(report.total_chunks_moved() > 0, "the group chunk moves to the receiver");

    // BOTH records land (the whole list was threaded, not just the first id) and the
    // two cores agree (true convergence).
    let recv_rows = query_tasks(&mut receiver);
    let send_rows = query_tasks(&mut sender);
    assert_eq!(recv_rows, send_rows, "cores converge after a transact group");
    assert_eq!(recv_rows.len(), 2, "both records imported: {recv_rows:?}");
    assert_eq!(recv_rows[0].0, "task-1");
    assert_eq!(recv_rows[0].1["title"], json!("first"));
    assert_eq!(recv_rows[1].0, "task-2");
    assert_eq!(recv_rows[1].1["title"], json!("second"));

    // The chunk histories are byte-identical (true convergence, not just projection).
    let doc = forge_storage::collection_doc_id("tasks");
    let recv_chunks: Vec<Vec<u8>> = receiver
        .store()
        .get_chunks(&doc)
        .unwrap()
        .into_iter()
        .map(|c| c.payload)
        .collect();
    let send_chunks: Vec<Vec<u8>> = sender
        .store()
        .get_chunks(&doc)
        .unwrap()
        .into_iter()
        .map(|c| c.payload)
        .collect();
    assert_eq!(recv_chunks, send_chunks, "chunk histories converge byte-identically");

    // The group was authorized (a concrete record identity was present), not denied.
    let allowed = receiver
        .events()
        .events_of_kind("sync.authorized")
        .next()
        .expect("the transact group was authorized");
    assert_eq!(allowed.payload["decision"], json!("allow"));
    assert_eq!(allowed.payload["collection"], json!("tasks"));
}

/// Issue a command as an Owner dev actor (the schema commands are command-level RBAC
/// gated to Owner/Maintainer; this is the LOCAL author boundary, distinct from the
/// SYNC apply boundary the membership table governs).
fn owner_cmd(name: &str, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req"),
        name: name.into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("dev"),
        workspace_id: WorkspaceId::new("ws"),
        payload,
    }
}

/// Apply a `schema.apply_change` on `core`, asserting it succeeded.
fn apply_change(core: &mut WorkspaceCore, change: Value) {
    let resp = core.handle(owner_cmd("schema.apply_change", json!({ "change": change })));
    assert!(resp.ok, "schema.apply_change failed: {:?}", resp.error);
}

/// Seed an `expenses` collection on `core` with an INDEXED int `amount` field, two
/// records (10/20), then widen `amount` int → float through the REAL
/// `schema.apply_change` command — which drives the durable DL-13 migration AND carries
/// the evolved registry collection onto the migration chunk (review 143). Leaves the
/// sender at `schema_version 2` with float records + the evolved registry.
fn seed_and_widen_expenses(core: &mut WorkspaceCore) {
    let idx = IndexManager::new();
    apply_change(core, json!({ "op": "add_collection", "name": "expenses" }));
    apply_change(
        core,
        json!({
            "op": "add_field",
            "collection": "expenses",
            "actor": "alice",
            "name": "amount",
            "ty": "int_num",
            "indexed": true,
            "required": false,
        }),
    );
    let expense = |id: &str, amount: i64, at: i64| Mutation::Insert {
        collection: "expenses".into(),
        id: Some(id.into()),
        fields: json!({ "amount": amount }).as_object().unwrap().clone(),
        logical_at: Some(at),
    };
    core.store_mut().apply_mutation_crdt(&expense("e1", 10, 1), &idx).unwrap();
    core.store_mut().apply_mutation_crdt(&expense("e2", 20, 2), &idx).unwrap();
    // The widen is keyed by the registry stable id `f_alice_0`; the companion migration
    // rewrites records keyed by the `f_amount` stand-in (the M0a record-side id). Each
    // accepted change advances the schema_version by one (add_collection, add_field, and
    // the widen migration), so the sender ends past v1 with the evolved registry; the
    // exact value is asserted only relative to the receiver post-sync (convergence).
    apply_change(
        core,
        json!({
            "op": "widen_field",
            "collection": "expenses",
            "field_id": "f_alice_0",
            "to": "float_num",
        }),
    );
    assert!(core.store().schema_version().unwrap() > 1, "sender advanced past v1");
}

#[test]
fn migration_chunk_denied_for_editor_without_schema_write() {
    // Review 143 (FIX 1, the DENY polarity): a DL-13 migration chunk is a SCHEMA-AFFECTING
    // op, not a plain record write. An Editor trusted with db.write on `expenses` but
    // WITHOUT schema_write may author ordinary record writes there — but the migration
    // needs schema-change authority (Owner/Maintainer + schema_write). It must be DENIED
    // BEFORE any chunk import or version advance, fail-closed. (This is the bypass the
    // prior regression mis-codified: an editor with schema_write:false asserting allow.)
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["expenses"]));
    seed_and_widen_expenses(&mut sender);
    assert_eq!(receiver.store().schema_version().unwrap(), 1, "receiver starts at v1");

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(
        report.chunks_denied, 1,
        "exactly the migration chunk must be DENIED (the plain record-insert chunks the \
         editor IS allowed to write still apply): report {report:?}"
    );

    // The migration's EFFECT is rejected before any schema state change: the
    // schema_version did NOT advance and the registry was NOT evolved. The receiver
    // sees the PRE-migration record state (the plain int inserts the editor may author),
    // proving the records were NOT carried forward by the denied migration.
    let rows = query_collection(&mut receiver, "expenses");
    assert_eq!(rows.len(), 2, "the editor's plain record writes still apply: {rows:?}");
    assert_eq!(rows[0].1["amount"], json!(10), "e1 stays the PRE-migration int (migration denied)");
    assert_eq!(rows[1].1["amount"], json!(20), "e2 stays the PRE-migration int (migration denied)");
    assert_eq!(
        receiver.store().schema_version().unwrap(),
        1,
        "the receiver's schema_version must NOT advance for an unauthorized migration"
    );
    assert!(
        receiver.registry().collection("expenses").is_none(),
        "the receiver's registry must NOT be evolved by an unauthorized migration"
    );

    // A permission_denied audit names the missing schema_write grant.
    let denial = receiver
        .events()
        .events_of_kind("sync.permission_denied")
        .find(|e| {
            e.payload["collection"] == json!("expenses")
                && e.payload["reason"].as_str().is_some_and(|r| r.contains("schema_write"))
        })
        .expect("a denial was audited naming the missing schema_write for the migration chunk");
    assert_eq!(denial.payload["decision"], json!("deny"));
}

#[test]
fn migration_chunk_authorized_for_maintainer_syncs_records_version_registry_and_index() {
    // Review 143 (FIX 1 allow polarity + FIX 2): a Maintainer trusted WITH schema_write
    // AND db.write on `expenses` may apply a migration. After an authorized sync the
    // receiver must converge on ALL of: the migrated record values, the schema_version,
    // the EVOLVED registry (not a stale one), AND the reconstructed indexed field — all
    // in lockstep, no drift.
    let (mut sender, mut receiver) = cores_with_membership(membership_full(
        "actor-maintainer",
        Role::Maintainer,
        &["expenses"],
        true,
    ));
    seed_and_widen_expenses(&mut sender);
    assert_eq!(receiver.store().schema_version().unwrap(), 1, "receiver starts at v1");
    assert!(receiver.registry().collection("expenses").is_none(), "receiver registry starts empty");

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(
        report.chunks_denied, 0,
        "an authorized migration must NOT be denied (report {report:?})"
    );
    assert!(report.total_chunks_moved() > 0, "the migration chunk moves to the receiver");

    // (records) The receiver holds the MIGRATED (float) values.
    let rows = query_collection(&mut receiver, "expenses");
    assert_eq!(rows.len(), 2, "both migrated records imported: {rows:?}");
    assert_eq!(rows[0].0, "e1");
    assert_eq!(rows[0].1["amount"], json!(10.0), "e1 migrated to float on the receiver");
    assert_eq!(rows[1].1["amount"], json!(20.0), "e2 migrated to float on the receiver");

    // (version) The receiver advanced to the migration target and CONVERGES with the
    // sender's version — no drift between version and the data/registry it describes.
    assert!(receiver.store().schema_version().unwrap() > 1, "the receiver advanced past v1");
    assert_eq!(
        receiver.store().schema_version().unwrap(),
        sender.store().schema_version().unwrap(),
        "the receiver converges on the sender's schema_version"
    );

    // (registry) The receiver's registry was EVOLVED in lockstep: it now KNOWS the
    // `expenses` collection and `amount` is the WIDENED float type — not stale/empty.
    let recv_col = receiver
        .registry()
        .collection("expenses")
        .expect("the receiver's registry now contains the synced collection");
    let amount = recv_col
        .field("f_alice_0")
        .expect("the receiver's registry contains the migrated field");
    assert_eq!(amount.name(), "amount");
    assert_eq!(
        *amount.ty(),
        forge_schema::FieldType::FloatNum,
        "the receiver's registry reflects the WIDENED type (registry synced, not stale)"
    );
    assert!(amount.indexed(), "the receiver's registry keeps the field indexed");
    // The sender and receiver registries agree byte-for-byte (true convergence).
    assert_eq!(
        receiver.registry().collection("expenses"),
        sender.registry().collection("expenses"),
        "sender and receiver registries converge"
    );

    // (index reconstruction) The receiver reconstructed the indexed field over the
    // `f_amount` stand-in from the evolved registry: the `Value` expression index for
    // `expenses.f_amount` is a live planner candidate after sync, proving the indexed
    // field was rebuilt (collection_indexed_fields keyed by f_<name>).
    assert!(
        receiver.indexes().get_expression("expenses", "f_amount").is_some(),
        "the receiver reconstructed the f_amount index from the evolved registry"
    );

    // The op was AUDITED as authorized (allow), naming the `expenses` collection.
    let allowed = receiver
        .events()
        .events_of_kind("sync.authorized")
        .find(|e| e.payload["collection"] == json!("expenses"))
        .expect("the migration chunk was authorized");
    assert_eq!(allowed.payload["decision"], json!("allow"));
}

/// Advance `core`'s workspace-GLOBAL `schema_version` to `target` by applying
/// schema changes on an UNRELATED collection (`vendors`, distinct from the `expenses`
/// collection the migration under test evolves) through the REAL `schema.apply_change`
/// command. Each accepted change advances the single workspace-wide counter by one
/// (`current + 1`), so this drives the global version forward WITHOUT ever touching the
/// `expenses` registry entry. Returns once `core.schema_version() == target`.
///
/// This reproduces the precondition for the review-w9 P1 bug: a receiver whose global
/// `schema_version` already equals the incoming migration's target — reached via work on
/// OTHER collections — so `advance_schema_version_if_newer` returns `advanced == false`
/// at import and the (previously gated) per-collection registry merge would be SKIPPED.
fn bump_schema_version_with_unrelated_work(core: &mut WorkspaceCore, target: u64) {
    assert!(
        core.store().schema_version().unwrap() <= target,
        "the receiver must start at or below the target before unrelated bumps"
    );
    // The first unrelated bump adds the `vendors` collection; each subsequent bump adds a
    // distinct field under actor `bob` (ids `f_bob_0`, `f_bob_1`, ...), all unrelated to
    // `expenses`/`f_alice_0`.
    if core.store().schema_version().unwrap() < target {
        apply_change(core, json!({ "op": "add_collection", "name": "vendors" }));
    }
    let mut seq = 0u64;
    while core.store().schema_version().unwrap() < target {
        apply_change(
            core,
            json!({
                "op": "add_field",
                "collection": "vendors",
                "actor": "bob",
                "name": format!("attr_{seq}"),
                "ty": "int_num",
                "indexed": false,
                "required": false,
            }),
        );
        seq += 1;
    }
    assert_eq!(
        core.store().schema_version().unwrap(),
        target,
        "the receiver's global schema_version was driven to the migration target via UNRELATED work"
    );
}

#[test]
fn migration_merges_registry_even_when_receiver_global_version_already_at_target() {
    // Review-w9 P1 (DL-13): the per-collection registry merge on a migration import must NOT
    // be gated on the workspace-GLOBAL `schema_version` actually moving forward. `schema_version`
    // is ONE workspace-wide counter, but registry evolution is PER-COLLECTION. A receiver whose
    // global version already reached the migration's target — via UNRELATED schema work on OTHER
    // collections — must STILL merge the carried `expenses` registry entry, or it imports the
    // migrated records while leaving its registry behind (data ahead of schema, the exact drift
    // class review 143 closed). The old code merged the registry only `if advanced`, so on this
    // receiver `advance_schema_version_if_newer` returned false and the merge was skipped.
    let (mut sender, mut receiver) = cores_with_membership(membership_full(
        "actor-maintainer",
        Role::Maintainer,
        &["expenses"],
        true,
    ));
    seed_and_widen_expenses(&mut sender);
    let target = sender.store().schema_version().unwrap();
    assert!(target > 1, "the sender authored the migration past v1");

    // Pre-bump the RECEIVER's global schema_version to the migration target using UNRELATED
    // work on `vendors`, so `advance_schema_version_if_newer` returns advanced=false at import.
    bump_schema_version_with_unrelated_work(&mut receiver, target);

    // PRECONDITION: the receiver is ALREADY at the target version but does NOT yet know the
    // migrated `expenses` collection (the unrelated work never touched it).
    assert_eq!(
        receiver.store().schema_version().unwrap(),
        target,
        "the receiver's global version already equals the migration target BEFORE the sync"
    );
    assert!(
        receiver.registry().collection("expenses").is_none(),
        "the receiver does NOT have the migrated `expenses` collection before the sync"
    );

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(
        report.chunks_denied, 0,
        "the authorized migration must NOT be denied even though the receiver is already at target \
         (report {report:?})"
    );
    assert!(report.total_chunks_moved() > 0, "the migration chunk moves to the receiver");

    // (registry) The crux: the receiver MERGED the carried `expenses` registry entry EVEN THOUGH
    // its global `schema_version` did not advance — the per-collection merge is no longer gated on
    // the global advance. It now KNOWS the `expenses` collection with the WIDENED float `amount`.
    let recv_col = receiver
        .registry()
        .collection("expenses")
        .expect("the receiver merged the carried `expenses` registry entry despite no global advance");
    let amount = recv_col
        .field("f_alice_0")
        .expect("the receiver's registry contains the migrated field");
    assert_eq!(amount.name(), "amount");
    assert_eq!(
        *amount.ty(),
        forge_schema::FieldType::FloatNum,
        "the receiver's registry reflects the WIDENED type (registry synced, not stale)"
    );
    assert!(amount.indexed(), "the receiver's registry keeps the field indexed");
    assert_eq!(
        receiver.registry().collection("expenses"),
        sender.registry().collection("expenses"),
        "sender and receiver `expenses` registries converge"
    );

    // (index reconstruction) The receiver reconstructed the `f_amount` index from the evolved
    // registry — proving the registry merge ran, not just the record import.
    assert!(
        receiver.indexes().get_expression("expenses", "f_amount").is_some(),
        "the receiver reconstructed the f_amount index from the merged registry"
    );

    // (records) The migrated (float) record values landed.
    let rows = query_collection(&mut receiver, "expenses");
    assert_eq!(rows.len(), 2, "both migrated records imported: {rows:?}");
    assert_eq!(rows[0].0, "e1");
    assert_eq!(rows[0].1["amount"], json!(10.0), "e1 migrated to float on the receiver");
    assert_eq!(rows[1].1["amount"], json!(20.0), "e2 migrated to float on the receiver");

    // (version) The receiver's global version stays at the target — the migration did not need to
    // advance it (it was already there), which is precisely why the buggy gate skipped the merge.
    assert_eq!(
        receiver.store().schema_version().unwrap(),
        target,
        "the receiver's global schema_version is unchanged (already at target — the gate trap)"
    );

    // A re-sync is an idempotent no-op: no chunks move, the registry is unchanged, and no error.
    let registry_before = receiver.registry().collection("expenses").cloned();
    let again = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(again.total_chunks_moved(), 0, "converged: the migration chunk does not re-move");
    assert_eq!(again.chunks_denied, 0, "the converged re-sync denies nothing");
    assert_eq!(
        receiver.registry().collection("expenses"),
        registry_before.as_ref(),
        "the re-sync left the merged `expenses` registry entry byte-identical (idempotent)"
    );
}

/// Build three empty cores A (author) → B (relay) → C (final receiver) with distinct
/// Loro peer ids, plus the symmetric back-channel trust each pair needs so only the
/// forward direction under test can deny. `b_trusts_a` / `c_trusts_a` decide whether the
/// migration A authors is authorized as a schema change at each hop (gated against A, the
/// ORIGINAL author, by provenance). Returns `(a, b, c)`.
fn three_relay_cores(
    b_trusts_a: TrustedMembership,
    c_trusts_a: TrustedMembership,
) -> (WorkspaceCore, WorkspaceCore, WorkspaceCore) {
    const A_PEER: u64 = 710;
    const B_PEER: u64 = 720;
    const C_PEER: u64 = 730;
    let mut a = WorkspaceCore::in_memory("ws-relay-a").unwrap();
    let mut b = WorkspaceCore::in_memory("ws-relay-b").unwrap();
    let mut c = WorkspaceCore::in_memory("ws-relay-c").unwrap();
    a.store_mut().set_crdt_peer_id(A_PEER);
    b.store_mut().set_crdt_peer_id(B_PEER);
    c.store_mut().set_crdt_peer_id(C_PEER);

    // B trusts A for the migration (A->B hop); C trusts A for the relayed migration
    // (B->C hop, authorized against the ORIGINAL author A — provenance).
    b.set_peer_membership(source_id_for(A_PEER), b_trusts_a).unwrap();
    c.set_peer_membership(source_id_for(A_PEER), c_trusts_a).unwrap();
    // C must also trust the relay B (the direct sender) as an owner; provenance gates
    // A's chunk against A, but B's own local writes (none here) would gate against B.
    c.set_peer_membership(
        source_id_for(B_PEER),
        membership("actor-b", Role::Owner, &["*"]),
    )
    .unwrap();
    // Symmetric back-channels so the reverse direction never spuriously denies.
    a.set_peer_membership(
        source_id_for(B_PEER),
        membership("actor-b", Role::Owner, &["*"]),
    )
    .unwrap();
    b.set_peer_membership(
        source_id_for(C_PEER),
        membership("actor-c", Role::Owner, &["*"]),
    )
    .unwrap();
    (a, b, c)
}

#[test]
fn migration_relays_through_two_hops_carrying_version_registry_and_index() {
    // Review 145 (P1): a genuine THREE-peer relay A -> B -> C. A authors a DL-13 migration
    // (widen `amount` int -> float via the real `schema.apply_change`); B imports it (A->B),
    // recording it as a `record.remote_import` row. When B RELAYS to C (B->C), the migration
    // metadata MUST survive that hop: B's remote-import row now carries the target
    // schema_version + the evolved registry entry + an is-migration marker, so the sync seam
    // re-stages the chunk as a SCHEMA-AFFECTING op (re-authorized as schema_write at the B->C
    // hop too), and C converges on the migrated records, schema_version, registry, AND the
    // reconstructed index — exactly like the direct A->B receiver. Before the fix B's relay
    // row dropped the version/registry, so C imported the migrated DATA as a plain record
    // write and stayed at the old schema_version with an unevolved registry (C inconsistent).
    let maintainer = || membership_full("actor-maintainer", Role::Maintainer, &["expenses"], true);
    let (mut a, mut b, mut c) = three_relay_cores(maintainer(), maintainer());
    seed_and_widen_expenses(&mut a);
    assert!(a.store().schema_version().unwrap() > 1, "A authored the migration");
    assert_eq!(b.store().schema_version().unwrap(), 1, "B starts at v1");
    assert_eq!(c.store().schema_version().unwrap(), 1, "C starts at v1");

    // Hop 1: A -> B. B authorizes the migration (Maintainer + schema_write) and converges.
    let a_to_b = a.sync_with(&mut b).unwrap();
    assert_eq!(a_to_b.chunks_denied, 0, "A->B: the authorized migration applies (report {a_to_b:?})");
    assert!(b.store().schema_version().unwrap() > 1, "B advanced on the A->B hop");
    // B holds the MIGRATED values after hop 1 (sanity — B is a faithful receiver).
    assert_eq!(query_collection(&mut b, "expenses")[0].1["amount"], json!(10.0));

    // Hop 2: B -> C. B only RELAYED A's migration (its oplog row is a record.remote_import).
    // The metadata must survive this hop, so C converges identically.
    let b_to_c = b.sync_with(&mut c).unwrap();
    assert_eq!(
        b_to_c.chunks_denied, 0,
        "B->C: the relayed migration must be re-authorized + applied, not dropped (report {b_to_c:?})"
    );
    assert!(b_to_c.total_chunks_moved() > 0, "the relayed migration chunk moves B -> C");

    // (records) C holds the MIGRATED (float) values — the migration reached the THIRD peer.
    let rows = query_collection(&mut c, "expenses");
    assert_eq!(rows.len(), 2, "both migrated records reached C: {rows:?}");
    assert_eq!(rows[0].0, "e1");
    assert_eq!(rows[0].1["amount"], json!(10.0), "e1 migrated to float on C (the relayed receiver)");
    assert_eq!(rows[1].1["amount"], json!(20.0), "e2 migrated to float on C");

    // (version) C advanced to the migration target and CONVERGES with A — through TWO hops.
    assert!(c.store().schema_version().unwrap() > 1, "C advanced past v1 via the relay");
    assert_eq!(
        c.store().schema_version().unwrap(),
        a.store().schema_version().unwrap(),
        "C converges on A's schema_version across two relay hops (no drift)"
    );

    // (registry) C's registry was EVOLVED in lockstep: it KNOWS the `expenses` collection
    // and `amount` is the WIDENED float type — the registry survived the relay hop, not stale.
    let recv_col = c
        .registry()
        .collection("expenses")
        .expect("C's registry contains the relayed collection");
    let amount = recv_col
        .field("f_alice_0")
        .expect("C's registry contains the migrated field");
    assert_eq!(amount.name(), "amount");
    assert_eq!(
        *amount.ty(),
        forge_schema::FieldType::FloatNum,
        "C's registry reflects the WIDENED type (registry metadata survived the relay)"
    );
    assert!(amount.indexed(), "C's registry keeps the field indexed");
    assert_eq!(
        c.registry().collection("expenses"),
        a.registry().collection("expenses"),
        "A and C registries converge across two relay hops"
    );

    // (index reconstruction) C reconstructed the `f_amount` index from the evolved registry.
    assert!(
        c.indexes().get_expression("expenses", "f_amount").is_some(),
        "C reconstructed the f_amount index from the relayed registry"
    );

    // C audited the relayed migration as authorized, gated against the ORIGINAL author A.
    let allowed = c
        .events()
        .events_of_kind("sync.authorized")
        .find(|e| e.payload["collection"] == json!("expenses"))
        .expect("C authorized the relayed migration chunk");
    assert_eq!(allowed.payload["decision"], json!("allow"));
    assert_eq!(
        allowed.payload["source"],
        json!(source_id_for(710)),
        "the relayed migration is authorized against the ORIGINAL author A, not relay B"
    );

    // A converged re-sync is a pure no-op AND leaves C's schema state unchanged.
    let again = b.sync_with(&mut c).unwrap();
    assert_eq!(again.total_chunks_moved(), 0, "converged: the relayed migration does not re-move");
    assert_eq!(c.store().schema_version().unwrap(), a.store().schema_version().unwrap());
}

#[test]
fn relayed_migration_is_denied_at_a_hop_without_schema_write() {
    // Review 145 (fail-closed twin): the schema_write gate must re-apply at EVERY relay hop,
    // not just the first. A authors a migration; B imports it (A->B authorized). B then relays
    // to C, but C trusts the ORIGINAL author A only as an EDITOR with db.write on `expenses`
    // and WITHOUT schema_write. Because B's relay row carries the schema-affecting metadata
    // forward (review 145), C sees the relayed chunk as a MIGRATION and DENIES it fail-closed —
    // it must NOT slip through as a plain record write the editor's db.write would allow. The
    // bug this guards: a dropped metadata row would let an unauthorized hop launder a schema
    // bump.
    let migrator = membership_full("actor-maintainer", Role::Maintainer, &["expenses"], true);
    // C trusts A as an Editor on `expenses` but WITHOUT schema_write.
    let c_trusts_a_editor = membership("actor-a-editor", Role::Editor, &["expenses"]);
    let (mut a, mut b, mut c) = three_relay_cores(migrator, c_trusts_a_editor);
    seed_and_widen_expenses(&mut a);

    // Hop 1: A -> B authorized.
    let a_to_b = a.sync_with(&mut b).unwrap();
    assert_eq!(a_to_b.chunks_denied, 0, "A->B: the migration is authorized at the first hop");
    assert!(b.store().schema_version().unwrap() > 1, "B advanced on the first hop");

    // Hop 2: B -> C. The relayed migration is DENIED at C (the editor lacks schema_write),
    // EVEN THOUGH the editor IS allowed plain db.write on `expenses` — proving the metadata
    // survived the relay so the gate still sees a migration, and the gate re-applies at C.
    let b_to_c = b.sync_with(&mut c).unwrap();
    assert_eq!(
        b_to_c.chunks_denied, 1,
        "B->C: the relayed migration must be DENIED at the unauthorized hop (report {b_to_c:?})"
    );

    // C's schema state is untouched: the migration's effect was rejected before any import.
    // The plain int record-insert chunks the editor IS allowed to write still applied, so C
    // sees the PRE-migration int values, NOT the migrated floats.
    let rows = query_collection(&mut c, "expenses");
    assert_eq!(rows.len(), 2, "the editor's plain record writes still apply at C: {rows:?}");
    assert_eq!(rows[0].1["amount"], json!(10), "e1 stays the PRE-migration int (migration denied at C)");
    assert_eq!(rows[1].1["amount"], json!(20), "e2 stays the PRE-migration int (migration denied at C)");
    assert_eq!(
        c.store().schema_version().unwrap(),
        1,
        "C's schema_version must NOT advance for the unauthorized relayed migration"
    );
    assert!(
        c.registry().collection("expenses").is_none(),
        "C's registry must NOT be evolved by the unauthorized relayed migration"
    );

    // C audited a permission_denied naming the missing schema_write for the relayed migration,
    // gated against the ORIGINAL author A (provenance), not the relay B.
    let denial = c
        .events()
        .events_of_kind("sync.permission_denied")
        .find(|e| {
            e.payload["collection"] == json!("expenses")
                && e.payload["reason"].as_str().is_some_and(|r| r.contains("schema_write"))
        })
        .expect("C audited a denial naming the missing schema_write for the relayed migration");
    assert_eq!(denial.payload["decision"], json!("deny"));
    assert_eq!(
        denial.payload["source"],
        json!(source_id_for(710)),
        "the denial is gated against the ORIGINAL author A, not relay B"
    );
}

#[test]
fn seeded_membership_survives_reopen() {
    // The SS-7 membership table is persisted to the workspace file (mirrors the
    // db.read grant table, review 050): a seeded row must survive `open(...)`,
    // not fail-open or revert to "unknown peer".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ws.forge");
    let row = membership("actor-editor", Role::Editor, &["tasks"]);
    {
        let mut core = WorkspaceCore::open(&path, "ws").unwrap();
        core.set_peer_membership(source_id_for(SENDER_PEER), row.clone())
            .unwrap();
    }
    // Reopen from the same file — the membership row is still trusted.
    let reopened = WorkspaceCore::open(&path, "ws").unwrap();
    assert_eq!(
        reopened.peer_membership(&source_id_for(SENDER_PEER)),
        Some(&row),
        "the seeded membership row must persist across reopen"
    );
}

// ---------------------------------------------------------------------------
// Data-driven provenance corpus (fixtures/sync-provenance, review 092 #1).
//
// Each vector drives the full three-peer relay topology through the PUBLIC
// `sync_with` path: author C writes, relay A imports C's chunk (A trusts C), then
// A syncs to B. B's trust for the relay A AND the original author C decides the
// forwarded chunk's fate. The runner asserts B's apply decision, B's visible
// projection, and — for a denial — that the audit names the ORIGINAL author's
// source, not the relay. The `relay_authored_locally_*` controls exercise the
// direct-author path (A authored the chunk itself; B authorizes A) so provenance
// handling cannot weaken or widen the non-forwarded case.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ProvFixtureOp {
    op: String,
    id: String,
    fields: Value,
}

#[derive(serde::Deserialize)]
struct ProvTrust {
    role: String,
    db_write: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ProvExpectRecord {
    id: String,
    fields: Value,
}

#[derive(serde::Deserialize)]
struct ProvExpect {
    decision: String,
    denied_count: usize,
    receiver_visible: Vec<ProvExpectRecord>,
    denial_source_is_author: bool,
}

#[derive(serde::Deserialize)]
struct ProvFixture {
    case: String,
    collection: String,
    author_peer: u64,
    relay_peer: u64,
    receiver_peer: u64,
    author_actor: String,
    relay_actor: String,
    author_ops: Vec<ProvFixtureOp>,
    relay_local_ops: Vec<ProvFixtureOp>,
    relay_trusts_author: ProvTrust,
    receiver_trusts_relay: ProvTrust,
    receiver_trusts_author: Option<ProvTrust>,
    expect: ProvExpect,
}

/// Parse a fixture role string into a [`Role`] (the fixtures spell roles in
/// PascalCase for readability; this maps them explicitly so a typo fails loudly).
fn parse_role(s: &str) -> Role {
    match s {
        "Owner" => Role::Owner,
        "Maintainer" => Role::Maintainer,
        "Editor" => Role::Editor,
        "Runner" => Role::Runner,
        "Viewer" => Role::Viewer,
        "Auditor" => Role::Auditor,
        "Reviewer" => Role::Reviewer,
        other => panic!("unknown role {other:?} in provenance fixture"),
    }
}

fn trust_to_membership(actor: &str, t: &ProvTrust) -> TrustedMembership {
    TrustedMembership {
        actor_id: actor.into(),
        role: parse_role(&t.role),
        db_read: vec!["*".into()],
        db_write: t.db_write.clone(),
        schema_write: false,
    }
}

fn prov_op_to_mutation(collection: &str, op: &ProvFixtureOp, at: i64) -> Mutation {
    match op.op.as_str() {
        "insert" => Mutation::Insert {
            collection: collection.into(),
            id: Some(op.id.clone()),
            fields: op.fields.as_object().expect("fields object").clone(),
            logical_at: Some(at),
        },
        other => panic!("provenance fixtures only use insert, got {other:?}"),
    }
}

fn query_collection(core: &mut WorkspaceCore, collection: &str) -> Vec<(String, Value)> {
    let cmd = CoreCommand {
        request_id: RequestId::new("req"),
        name: "query.execute".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("dev"),
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({ "collection": collection }),
    };
    let resp = core.handle(cmd);
    assert!(resp.ok, "query.execute failed: {:?}", resp.error);
    let mut rows: Vec<(String, Value)> = resp.payload["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .map(|r| (r["id"].as_str().unwrap().to_string(), r["fields"].clone()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn run_provenance_fixture(fx: &ProvFixture) {
    let idx = IndexManager::new();
    let mut c = WorkspaceCore::in_memory("ws-prov-c").unwrap();
    let mut a = WorkspaceCore::in_memory("ws-prov-a").unwrap();
    let mut b = WorkspaceCore::in_memory("ws-prov-b").unwrap();
    c.store_mut().set_crdt_peer_id(fx.author_peer);
    a.store_mut().set_crdt_peer_id(fx.relay_peer);
    b.store_mut().set_crdt_peer_id(fx.receiver_peer);

    let author_src = source_id_for(fx.author_peer);
    let relay_src = source_id_for(fx.relay_peer);
    let receiver_src = source_id_for(fx.receiver_peer);

    // A trusts C so C -> A applies and A becomes a relay for C's chunk.
    a.set_peer_membership(
        author_src.clone(),
        trust_to_membership(&fx.author_actor, &fx.relay_trusts_author),
    )
    .unwrap();
    // Back-channel trust so the symmetric directions never spuriously deny.
    c.set_peer_membership(relay_src.clone(), membership(&fx.relay_actor, Role::Owner, &["*"]))
        .unwrap();
    a.set_peer_membership(receiver_src.clone(), membership("actor-b", Role::Owner, &["*"]))
        .unwrap();
    // B trusts the relay A as configured, and the author C only if the fixture seeds it.
    b.set_peer_membership(
        relay_src.clone(),
        trust_to_membership(&fx.relay_actor, &fx.receiver_trusts_relay),
    )
    .unwrap();
    if let Some(t) = &fx.receiver_trusts_author {
        b.set_peer_membership(author_src.clone(), trust_to_membership(&fx.author_actor, t))
            .unwrap();
    }

    // C authors its ops; A imports them as a relay.
    let mut clock = 0i64;
    for op in &fx.author_ops {
        clock += 1;
        c.store_mut()
            .apply_mutation_crdt(&prov_op_to_mutation(&fx.collection, op, clock), &idx)
            .unwrap();
    }
    if !fx.author_ops.is_empty() {
        c.sync_with(&mut a).unwrap();
    }
    // A also authors its OWN ops (the relay-as-author control path).
    for op in &fx.relay_local_ops {
        clock += 1;
        a.store_mut()
            .apply_mutation_crdt(&prov_op_to_mutation(&fx.collection, op, clock), &idx)
            .unwrap();
    }

    // The hop under test: A -> B. B authorizes each chunk against its ORIGINAL author.
    let report = a.sync_with(&mut b).unwrap();
    assert_eq!(
        report.chunks_denied, fx.expect.denied_count,
        "case {}: denied count mismatch (report {report:?})",
        fx.case
    );

    // B's visible projection must equal the fixture's expectation exactly.
    let got = query_collection(&mut b, &fx.collection);
    assert_eq!(
        got.len(),
        fx.expect.receiver_visible.len(),
        "case {}: B record count mismatch (got {got:?})",
        fx.case
    );
    for want in &fx.expect.receiver_visible {
        let have = got
            .iter()
            .find(|(id, _)| id == &want.id)
            .unwrap_or_else(|| panic!("case {}: B missing record {}", fx.case, want.id));
        let want_fields = want.fields.as_object().expect("expected fields object");
        for (k, v) in want_fields {
            assert_eq!(
                have.1.get(k),
                Some(v),
                "case {}: B record {} field {k} mismatch",
                fx.case,
                want.id
            );
        }
    }

    match fx.expect.decision.as_str() {
        "applied" => {
            let allowed = b
                .events()
                .events_of_kind("sync.authorized")
                .next()
                .unwrap_or_else(|| panic!("case {}: expected an allow audit", fx.case));
            // An applied forwarded chunk is authorized against the ORIGINAL author.
            if !fx.author_ops.is_empty() {
                assert_eq!(
                    allowed.payload["source"], json!(author_src),
                    "case {}: allow must name the original author, not the relay",
                    fx.case
                );
            }
        }
        "permission_denied" => {
            let denial = b
                .events()
                .events_of_kind("sync.permission_denied")
                .next()
                .unwrap_or_else(|| panic!("case {}: expected a denial audit", fx.case));
            assert_eq!(denial.payload["decision"], json!("deny"), "case {}", fx.case);
            let expected_source = if fx.expect.denial_source_is_author {
                &author_src
            } else {
                &relay_src
            };
            assert_eq!(
                denial.payload["source"], json!(expected_source),
                "case {}: denial must name the {} source",
                fx.case,
                if fx.expect.denial_source_is_author { "original author" } else { "relay" }
            );
        }
        other => panic!("case {}: unknown expected decision {other:?}", fx.case),
    }
}

#[derive(serde::Deserialize)]
struct ProvManifest {
    count: usize,
}

fn provenance_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sync-provenance")
}

#[test]
fn every_sync_provenance_vector_matches_expected_decision() {
    let dir = provenance_dir();
    let manifest: ProvManifest = serde_json::from_str(
        &std::fs::read_to_string(dir.join("manifest.json")).expect("read provenance manifest"),
    )
    .expect("parse provenance manifest");

    let mut ran = 0usize;
    for entry in std::fs::read_dir(&dir).expect("read sync-provenance dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") {
            continue;
        }
        let fx: ProvFixture = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display())),
        )
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()));
        run_provenance_fixture(&fx);
        ran += 1;
    }

    // Guard against a silently empty / partial run (e.g. a moved fixtures dir): the
    // suite is only load-bearing if it ran EVERY declared vector.
    assert_eq!(
        ran, manifest.count,
        "ran {ran} provenance vectors but the manifest declares {}",
        manifest.count
    );
}
