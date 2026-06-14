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
