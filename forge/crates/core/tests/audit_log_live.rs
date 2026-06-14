//! SC-12 LIVE WIRING: a real authorization denial lands a persisted, queryable
//! `audit_log` row through the production decision path — not a tested-but-
//! disconnected library (the db.watch lesson). These tests drive the PUBLIC
//! [`WorkspaceCore::handle`] / [`WorkspaceCore::sync_with`] surfaces and then
//! read the durable log back through [`forge_storage::Store::query_audit`].
//!
//! The acceptance bar (`forge/spec/audit-log.md`): "a real sync-RBAC /
//! command-RBAC denial lands a persisted, queryable row through the live decision
//! path." Each test makes a denial happen the way the application would, then
//! proves the row is in `audit_log`, queryable by the same filters a security
//! reviewer would use (`decision = deny`, `actor_id = ...`). It also asserts the
//! APPEND-ONLY invariant against the live path: re-running the same denial appends
//! a NEW row (fresh seq/audit_id) and never mutates the prior one.

use forge_core::{source_id_for, TrustedMembership, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use forge_storage::{AuditQuery, IndexManager, Mutation};
use serde_json::{json, Value};

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

/// A sender/receiver pair with distinct Loro peer ids; the receiver trusts the
/// sender exactly as `receiver_trusts` says.
fn cores_with_membership(receiver_trusts: TrustedMembership) -> (WorkspaceCore, WorkspaceCore) {
    let mut sender = WorkspaceCore::in_memory("ws-sender").unwrap();
    let mut receiver = WorkspaceCore::in_memory("ws-receiver").unwrap();
    sender.store_mut().set_crdt_peer_id(SENDER_PEER);
    receiver.store_mut().set_crdt_peer_id(RECEIVER_PEER);
    receiver
        .set_peer_membership(source_id_for(SENDER_PEER), receiver_trusts)
        .unwrap();
    sender
        .set_peer_membership(
            source_id_for(RECEIVER_PEER),
            membership("actor-receiver", Role::Owner, &["*"]),
        )
        .unwrap();
    (sender, receiver)
}

#[test]
fn sync_rbac_denial_persists_queryable_audit_row_through_live_path() {
    // The receiver trusts the sender only as a VIEWER (no write role): a remote
    // record insert MUST be denied by the live SS-7 apply gate.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-viewer", Role::Viewer, &[]));

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-x", json!({ "title": "viewer write" }), 1), &idx)
        .unwrap();

    // Drive the REAL sync path. The op is denied before import.
    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 1, "the viewer write is denied");
    assert_eq!(report.chunks_a_to_b, 0, "nothing imported into the receiver");

    // The denial landed a DURABLE row in the receiver's audit_log — queryable by
    // decision exactly as a security reviewer would search it.
    let denies = receiver
        .store()
        .query_audit(&AuditQuery::by_decision("deny"))
        .unwrap();
    assert_eq!(denies.len(), 1, "exactly one persisted deny row: {denies:?}");
    let row = &denies[0];
    assert_eq!(row.producer, "sync-rbac");
    assert_eq!(row.action, "sync.record.insert");
    assert_eq!(row.decision, "deny");
    // The `actor_id` is the TRUSTED membership row's actor (the authenticated
    // identity that decided the op), not the raw `peer:<id>` source — the source
    // is carried in metadata.
    assert_eq!(row.actor_id, "actor-viewer");
    assert_eq!(row.resource_type, "record");
    assert_eq!(row.collection.as_deref(), Some("tasks"));
    assert!(
        row.reason.contains("viewer"),
        "the persisted reason names the viewer role: {}",
        row.reason
    );
    // seq + audit_id are minted by the store (deterministic ordering key).
    assert_eq!(row.seq, 1);
    assert_eq!(row.audit_id, "audit-000001");
    // The metadata carries the TRUSTED grant snapshot + record ids (no secret/body).
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("trusted_role").unwrap(), "Viewer");
    assert_eq!(meta.get("record_ids").unwrap(), &json!(["task-x"]));

    // The SENDER's log holds nothing for this op — only the RECEIVER decides/records.
    assert!(
        sender
            .store()
            .query_audit(&AuditQuery::by_decision("deny"))
            .unwrap()
            .is_empty(),
        "the sender records no deny; the receiver owns the apply-boundary decision"
    );
}

#[test]
fn sync_rbac_denial_is_append_only_across_reruns_via_live_path() {
    // APPEND-ONLY against the LIVE path: re-running the same denied sync appends a
    // NEW row (fresh seq/audit_id) and never mutates the prior one.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-viewer", Role::Viewer, &[]));
    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-x", json!({ "title": "viewer write" }), 1), &idx)
        .unwrap();

    sender.sync_with(&mut receiver).unwrap();
    let first = receiver
        .store()
        .query_audit(&AuditQuery::by_decision("deny"))
        .unwrap();
    assert_eq!(first.len(), 1, "first denial recorded");
    let first_row = first[0].clone();

    // Author a SECOND distinct denied op and sync again. The first chunk was never
    // imported (it was denied), so a second sync RE-OFFERS task-x AND offers the new
    // task-y — both denied — so the receiver APPENDS two more rows. History only ever
    // grows; the prior row is never rewritten.
    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-y", json!({ "title": "again" }), 2), &idx)
        .unwrap();
    sender.sync_with(&mut receiver).unwrap();

    let after = receiver
        .store()
        .query_audit(&AuditQuery::by_decision("deny"))
        .unwrap();
    assert_eq!(
        after.len(),
        3,
        "the re-run APPENDED rows (re-offered task-x + new task-y), history grew: {after:?}"
    );
    // The prior row is byte-identical — untouched (no UPDATE/DELETE of history).
    assert_eq!(after[0].seq, first_row.seq);
    assert_eq!(after[0].audit_id, first_row.audit_id);
    assert_eq!(after[0].reason, first_row.reason);
    assert_eq!(after[0].metadata, first_row.metadata);
    // Each appended row has a strictly higher seq + distinct audit_id (gap-free).
    assert!(after[1].seq > after[0].seq, "seq strictly increases on append");
    assert!(after[2].seq > after[1].seq, "seq strictly increases on append");
    assert_ne!(after[1].audit_id, after[0].audit_id);
    assert_ne!(after[2].audit_id, after[1].audit_id);
}

#[test]
fn command_rbac_denial_persists_queryable_audit_row_through_live_path() {
    // An Auditor cannot `runtime.run` (read-only/oversight role) — the live CR-A3
    // command-RBAC gate denies it. The denial must land a queryable audit row.
    let mut core = WorkspaceCore::in_memory("ws-cmd").unwrap();
    let cmd = CoreCommand {
        request_id: RequestId::new("req-1"),
        name: "runtime.run".into(),
        applet_id: Some(AppletId::new("applet.notes")),
        actor: ActorContext {
            actor: "actor-auditor-1".into(),
            role: Role::Auditor,
        },
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({}),
    };
    let resp = core.handle(cmd);
    assert!(!resp.ok, "the auditor's runtime.run must be denied");

    // The denial is queryable by ACTOR (the security reviewer's lookup).
    let rows = core
        .store()
        .query_audit(&AuditQuery::by_actor("actor-auditor-1"))
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one persisted command-rbac deny: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.producer, "command-rbac");
    assert_eq!(row.action, "command.runtime.run");
    assert_eq!(row.decision, "deny");
    assert_eq!(row.actor_id, "actor-auditor-1");
    assert_eq!(row.resource_type, "command");
    assert_eq!(row.resource_id.as_deref(), Some("runtime.run"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("role").unwrap(), "Auditor");
    assert_eq!(meta.get("command").unwrap(), "runtime.run");
    assert_eq!(meta.get("applet_id").unwrap(), "applet.notes");

    // Cross-check it is ALSO findable by decision=deny (the same row).
    let by_decision = core
        .store()
        .query_audit(&AuditQuery::by_decision("deny"))
        .unwrap();
    assert_eq!(by_decision.len(), 1);
    assert_eq!(by_decision[0].audit_id, row.audit_id);
}

#[test]
fn command_rbac_denial_is_append_only_across_reruns_via_live_path() {
    // Re-issuing the SAME denied command appends a NEW row; the prior is untouched.
    let mut core = WorkspaceCore::in_memory("ws-cmd-append").unwrap();
    let denied = || CoreCommand {
        request_id: RequestId::new("req"),
        name: "applet.install".into(),
        applet_id: Some(AppletId::new("applet.notes")),
        actor: ActorContext {
            actor: "actor-viewer-1".into(),
            role: Role::Viewer,
        },
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({}),
    };
    assert!(!core.handle(denied()).ok, "a viewer cannot install");
    let after_one = core
        .store()
        .query_audit(&AuditQuery::by_actor("actor-viewer-1"))
        .unwrap();
    assert_eq!(after_one.len(), 1);
    let first = after_one[0].clone();

    assert!(!core.handle(denied()).ok, "a viewer still cannot install");
    let after_two = core
        .store()
        .query_audit(&AuditQuery::by_actor("actor-viewer-1"))
        .unwrap();
    assert_eq!(after_two.len(), 2, "the re-run appended a second row");
    // Prior row byte-identical.
    assert_eq!(after_two[0].seq, first.seq);
    assert_eq!(after_two[0].audit_id, first.audit_id);
    // New row strictly later.
    assert!(after_two[1].seq > after_two[0].seq);
    assert_ne!(after_two[1].audit_id, after_two[0].audit_id);
    assert_eq!(after_two[1].action, "command.applet.install");
}

#[test]
fn allowed_command_persists_no_command_rbac_deny_row() {
    // An ALLOWED command does not write a command-RBAC DENY row: only denials are
    // the command-RBAC producer (the live gate writes on the Err(PermissionDenied)
    // branch). This guards against the wiring spuriously logging every command.
    let mut core = WorkspaceCore::in_memory("ws-allow").unwrap();
    let cmd = CoreCommand {
        request_id: RequestId::new("req-ok"),
        name: "workspace.open".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("dev"),
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({}),
    };
    let resp = core.handle(cmd);
    assert!(resp.ok, "workspace.open is allowed for an owner");
    assert!(
        core.store()
            .query_audit(&AuditQuery::by_decision("deny"))
            .unwrap()
            .is_empty(),
        "an allowed command writes no command-rbac deny row"
    );
}
