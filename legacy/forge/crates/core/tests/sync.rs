//! Workspace-level in-process CRDT sync convergence (SS-1/SS-2, M0b).
//!
//! The proof that two whole [`WorkspaceCore`]s converge through the public
//! [`WorkspaceCore::sync_with`] seam: each core writes records into the same
//! collection under its own distinct Loro peer id — different record ids plus a
//! concurrent patch to DIFFERENT fields of a shared record — then `sync_with`
//! exchanges their CRDT chunks and rebuilds both projections. After sync each
//! core's `query.execute` returns the identical converged set (independent
//! records merge; the shared record carries both peers' fields). A second sync
//! is a no-op.

use forge_core::{source_id_for, TrustedMembership, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use forge_storage::{IndexManager, Mutation};
use serde_json::{json, Value};

/// An owner membership row (wildcard write) the SS-7 apply gate trusts for a peer.
fn owner_membership(actor: &str) -> TrustedMembership {
    TrustedMembership {
        actor_id: actor.into(),
        role: Role::Owner,
        db_read: vec!["*".into()],
        db_write: vec!["*".into()],
        schema_write: true,
    }
}

/// An owner command (owner permits the `query.execute` read; no db.read grant
/// needed for the role-derived read-all fallback).
fn query_cmd(collection: &str) -> CoreCommand {
    owner_cmd("query.execute", json!({ "collection": collection }))
}

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

/// Read a core's `tasks` projection back through the PUBLIC `query.execute`
/// command as a sorted `(id, fields)` list, so the assertion goes through the
/// real read surface rather than poking the store.
fn query_tasks(core: &mut WorkspaceCore) -> Vec<(String, Value)> {
    let resp = core.handle(query_cmd("tasks"));
    assert!(resp.ok, "query.execute failed: {:?}", resp.error);
    let mut rows: Vec<(String, Value)> = resp.payload["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .map(|r| {
            (
                r["id"].as_str().expect("id").to_string(),
                r["fields"].clone(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn insert(id: &str, fields: Value, at: i64) -> Mutation {
    Mutation::Insert {
        collection: "tasks".into(),
        id: Some(id.into()),
        fields: fields.as_object().unwrap().clone(),
        logical_at: Some(at),
    }
}

fn patch(id: &str, fields: Value, at: i64) -> Mutation {
    Mutation::Patch {
        collection: "tasks".into(),
        id: id.into(),
        fields: fields.as_object().unwrap().clone(),
        logical_at: Some(at),
    }
}

#[test]
fn two_cores_converge_through_sync_with() {
    let idx = IndexManager::new();

    // Two workspaces, each minting CRDT ops under a DISTINCT Loro peer id so a
    // concurrent same-record edit converges to one agreed winner.
    let mut peer_a = WorkspaceCore::in_memory("ws-a").unwrap();
    let mut peer_b = WorkspaceCore::in_memory("ws-b").unwrap();
    peer_a.store_mut().set_crdt_peer_id(101);
    peer_b.store_mut().set_crdt_peer_id(202);

    // SS-7: each peer seeds the OTHER as a trusted owner (wildcard write) in its
    // receiver-side membership table, keyed by the other's sync source id. Without
    // a trusted row the apply gate fail-closes and denies every incoming op.
    peer_a
        .set_peer_membership(source_id_for(202), owner_membership("actor-b"))
        .unwrap();
    peer_b
        .set_peer_membership(source_id_for(101), owner_membership("actor-a"))
        .unwrap();

    // A SHARED baseline record both peers start from: write it on A, sync it to
    // the (empty) B, so both hold the same baseline CRDT history before they
    // diverge (the concurrent patch then targets a genuinely shared record).
    peer_a
        .store_mut()
        .apply_mutation_crdt(
            &insert("shared", json!({"title": "shared", "status": "open"}), 1),
            &idx,
        )
        .unwrap();
    peer_a.sync_with(&mut peer_b).unwrap();

    // Divergent, concurrent writes:
    //  - each peer inserts its OWN record (different ids) — both must survive;
    //  - each peer patches a DIFFERENT field of the shared record — both fields
    //    must survive (not collide on one whole-record register).
    peer_a
        .store_mut()
        .apply_mutation_crdt(&insert("a1", json!({"title": "from-a"}), 2), &idx)
        .unwrap();
    peer_a
        .store_mut()
        .apply_mutation_crdt(&patch("shared", json!({"owner": "a"}), 3), &idx)
        .unwrap();

    peer_b
        .store_mut()
        .apply_mutation_crdt(&insert("b1", json!({"title": "from-b"}), 2), &idx)
        .unwrap();
    peer_b
        .store_mut()
        .apply_mutation_crdt(&patch("shared", json!({"pinned": true}), 3), &idx)
        .unwrap();

    // Converge.
    let report = peer_a.sync_with(&mut peer_b).unwrap();
    assert!(
        report.total_chunks_moved() > 0,
        "the first sync should move chunks"
    );

    // Both cores return the IDENTICAL converged set through query.execute.
    let a_rows = query_tasks(&mut peer_a);
    let b_rows = query_tasks(&mut peer_b);
    assert_eq!(a_rows, b_rows, "the two cores disagree after sync");

    // The converged content: three records; the shared one carries BOTH peers'
    // concurrent field patches (different-field merge survives).
    assert_eq!(a_rows.len(), 3, "shared + a1 + b1");
    let shared = &a_rows.iter().find(|(id, _)| id == "shared").unwrap().1;
    assert_eq!(shared["title"], json!("shared"));
    assert_eq!(shared["status"], json!("open"));
    assert_eq!(
        shared["owner"],
        json!("a"),
        "peer A's concurrent field survived"
    );
    assert_eq!(
        shared["pinned"],
        json!(true),
        "peer B's concurrent field survived"
    );
    assert_eq!(
        a_rows.iter().find(|(id, _)| id == "a1").unwrap().1["title"],
        json!("from-a")
    );
    assert_eq!(
        a_rows.iter().find(|(id, _)| id == "b1").unwrap().1["title"],
        json!("from-b")
    );

    // A second sync over the now-converged pair is a no-op (idempotent).
    let again = peer_a.sync_with(&mut peer_b).unwrap();
    assert_eq!(
        again.total_chunks_moved(),
        0,
        "a second sync must move no chunks"
    );
}

#[test]
fn sync_export_import_commands_move_chunks_through_core_boundary() {
    let idx = IndexManager::new();
    let mut peer_a = WorkspaceCore::in_memory("ws-a").unwrap();
    let mut peer_b = WorkspaceCore::in_memory("ws-b").unwrap();
    peer_a.store_mut().set_crdt_peer_id(101);
    peer_b.store_mut().set_crdt_peer_id(202);

    peer_a
        .store_mut()
        .apply_mutation_crdt(
            &insert("a1", json!({"title": "exported through command"}), 1),
            &idx,
        )
        .unwrap();

    let export = peer_a.handle(owner_cmd("sync.export", json!({})));
    assert!(export.ok, "sync.export failed: {:?}", export.error);
    let packet = export.payload["packet"].clone();
    let source = packet["source"].as_str().expect("source").to_string();
    assert_eq!(source, "peer:101");
    assert_eq!(packet["chunks"].as_array().unwrap().len(), 1);

    let denied = peer_b.handle(owner_cmd(
        "sync.import",
        json!({ "packet": packet.clone() }),
    ));
    assert!(
        denied.ok,
        "unknown-peer import should be a deny report, not a command error"
    );
    assert_eq!(denied.payload["chunks_imported"], json!(0));
    assert_eq!(denied.payload["chunks_denied"], json!(1));
    assert!(query_tasks(&mut peer_b).is_empty());

    let trust = peer_b.handle(owner_cmd(
        "sync.trust_peer",
        json!({
            "source": source,
            "membership": owner_membership("actor-a")
        }),
    ));
    assert!(trust.ok, "sync.trust_peer failed: {:?}", trust.error);

    let imported = peer_b.handle(owner_cmd(
        "sync.import",
        json!({ "packet": packet.clone() }),
    ));
    assert!(imported.ok, "trusted import failed: {:?}", imported.error);
    assert_eq!(imported.payload["chunks_seen"], json!(1));
    assert_eq!(imported.payload["chunks_imported"], json!(1));
    assert_eq!(imported.payload["chunks_denied"], json!(0));

    let rows = query_tasks(&mut peer_b);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "a1");
    assert_eq!(rows[0].1["title"], json!("exported through command"));

    let again = peer_b.handle(owner_cmd("sync.import", json!({ "packet": packet })));
    assert!(again.ok, "idempotent re-import failed: {:?}", again.error);
    assert_eq!(again.payload["chunks_imported"], json!(0));
    assert_eq!(again.payload["chunks_denied"], json!(0));
}
