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

use forge_core::{source_id_for, TrustedMembership, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use forge_storage::{IndexManager, Mutation};
use serde_json::{json, Value};

// Distinct Loro peer ids so concurrent edits would converge to one agreed winner.
const SENDER_PEER: u64 = 700;
const RECEIVER_PEER: u64 = 800;

fn membership(actor: &str, role: Role, db_write: &[&str]) -> TrustedMembership {
    TrustedMembership {
        actor_id: actor.into(),
        role,
        db_read: vec!["*".into()],
        db_write: db_write.iter().map(|s| s.to_string()).collect(),
        schema_write: false,
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
