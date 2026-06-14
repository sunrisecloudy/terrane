//! SC-12 `audit.query` command: the privileged READ over the durable audit log,
//! exercised through the LIVE [`WorkspaceCore::handle`] surface (not the raw
//! `Store::query_audit`). These tests prove the command boundary the storage layer
//! defers to:
//!
//! - a denied command (here, a Viewer's `runtime.run`) PERSISTS a command-RBAC row
//!   through the live path, and a privileged caller reads it back with `audit.query`
//!   — filtered by `decision`, `actor_id`, and `seq` range, ordered by `seq`;
//! - the read is PRIVILEGED: an oversight role (Auditor/Maintainer/Owner) may read
//!   the trail, but a data-only Editor/Viewer is denied — and that very denial lands
//!   a command-RBAC audit row, so an attempt to read the security log is itself
//!   auditable;
//! - the empty-result path returns an empty `rows` array (not an error);
//! - a malformed filter is a `ValidationError`, never a silently-widened query.
//!
//! Together with `audit_log_live.rs` (which proves the producers WRITE through the
//! live path) this proves the full SC-12 loop: real producers persist, the
//! `audit.query` command reads back, all through the production decision path.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, AppletId, CoreCommand, CoreResponse, RequestId, Role, WorkspaceId};
use forge_storage::AuditQuery;
use serde_json::{json, Value};

/// A `runtime.run` command for `actor` at `role` — denied for read-only roles, so it
/// is a convenient way to LAND a real command-RBAC deny row through the live path.
fn runtime_run(actor: &str, role: Role) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req-run"),
        name: "runtime.run".into(),
        applet_id: Some(AppletId::new("applet.notes")),
        actor: ActorContext { actor: actor.into(), role },
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({}),
    }
}

/// An `audit.query` command for `actor` at `role` with the given `filter` payload.
fn audit_query(actor: &str, role: Role, filter: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req-audit"),
        name: "audit.query".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext { actor: actor.into(), role },
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({ "filter": filter }),
    }
}

/// The `rows` array from a successful `audit.query` response.
fn rows(resp: &CoreResponse) -> &Vec<Value> {
    assert!(resp.ok, "audit.query should succeed: {:?}", resp.error);
    resp.payload["rows"].as_array().expect("rows is an array")
}

/// The `audit_id`s of a query result, in result order (seq ascending).
fn audit_ids(resp: &CoreResponse) -> Vec<String> {
    rows(resp)
        .iter()
        .map(|r| r["audit_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn audit_query_reads_back_a_live_command_rbac_denial_through_the_command_path() {
    let mut core = WorkspaceCore::in_memory("ws-audit-read").unwrap();

    // LAND a real command-RBAC deny row: a Viewer cannot runtime.run (read-only).
    let denied = core.handle(runtime_run("actor-viewer-1", Role::Viewer));
    assert!(!denied.ok, "the viewer's runtime.run is denied");

    // Read it back via the LIVE audit.query command as a privileged Auditor.
    let resp = core.handle(audit_query(
        "actor-auditor-1",
        Role::Auditor,
        json!({ "decision": "deny" }),
    ));
    let result = rows(&resp);
    assert_eq!(result.len(), 1, "exactly one persisted deny row: {result:?}");
    let row = &result[0];
    assert_eq!(row["producer"], "command-rbac");
    assert_eq!(row["action"], "command.runtime.run");
    assert_eq!(row["decision"], "deny");
    assert_eq!(row["actor_id"], "actor-viewer-1");
    assert_eq!(row["resource_type"], "command");
    assert_eq!(row["resource_id"], "runtime.run");
    // The full row shape is present, including the (redacted) metadata.
    assert_eq!(row["metadata"]["role"], "Viewer");
    assert_eq!(row["metadata"]["command"], "runtime.run");
    // seq + audit_id are the deterministic ordering key minted by the store.
    assert_eq!(row["seq"], 1);
    assert_eq!(row["audit_id"], "audit-000001");
    assert!(row["logical_time"].as_u64().is_some(), "logical_time present");
}

#[test]
fn audit_query_filters_by_actor_action_and_sequence_range() {
    let mut core = WorkspaceCore::in_memory("ws-audit-filter").unwrap();

    // Land three live denials from two distinct actors (seq 1, 2, 3).
    assert!(!core.handle(runtime_run("actor-viewer-1", Role::Viewer)).ok);
    assert!(!core.handle(runtime_run("actor-runner-x", Role::Reviewer)).ok); // Reviewer cannot runtime.run
    assert!(!core.handle(runtime_run("actor-viewer-1", Role::Viewer)).ok);

    // by actor: only actor-viewer-1's two rows.
    let by_actor = core.handle(audit_query(
        "owner",
        Role::Owner,
        json!({ "actor_id": "actor-viewer-1" }),
    ));
    assert_eq!(audit_ids(&by_actor), vec!["audit-000001", "audit-000003"]);

    // by action: all three are command.runtime.run.
    let by_action = core.handle(audit_query(
        "owner",
        Role::Owner,
        json!({ "action": "command.runtime.run" }),
    ));
    assert_eq!(
        audit_ids(&by_action),
        vec!["audit-000001", "audit-000002", "audit-000003"]
    );

    // by seq range (inclusive 2..=3).
    let by_range = core.handle(audit_query(
        "owner",
        Role::Owner,
        json!({ "seq_gte": 2, "seq_lte": 3 }),
    ));
    assert_eq!(audit_ids(&by_range), vec!["audit-000002", "audit-000003"]);

    // No filter → every row, seq-ordered. The three command-RBAC deny rows (seq
    // 1..=3) come FIRST, then the SELF-AUDIT `audit.query` allow rows that each prior
    // successful read appended (`by_actor` → seq 4, `by_action` → seq 5, `by_range` →
    // seq 6; review 150). The unfiltered read returns the snapshot BEFORE its own
    // allow row lands, so seq 7 (this read's row) is not in the result.
    let all = core.handle(CoreCommand {
        request_id: RequestId::new("req-all"),
        name: "audit.query".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("owner"),
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({}),
    });
    assert_eq!(
        audit_ids(&all),
        vec![
            "audit-000001",
            "audit-000002",
            "audit-000003",
            "audit-000004",
            "audit-000005",
            "audit-000006",
        ]
    );
    // The first three are the runtime.run denials; the rest are the audit.query
    // self-audit allow rows.
    let all_rows = rows(&all);
    for deny in &all_rows[0..3] {
        assert_eq!(deny["action"], "command.runtime.run");
        assert_eq!(deny["decision"], "deny");
    }
    for allow in &all_rows[3..6] {
        assert_eq!(allow["producer"], "audit");
        assert_eq!(allow["action"], "audit.query");
        assert_eq!(allow["decision"], "allow");
        assert_eq!(allow["resource_type"], "audit_log");
        assert_eq!(allow["resource_id"], "audit_log");
        assert_eq!(allow["actor_id"], "owner");
    }
}

#[test]
fn audit_query_empty_result_is_not_an_error() {
    let mut core = WorkspaceCore::in_memory("ws-audit-empty").unwrap();
    // A privileged read of an empty log returns an empty rows array, not an error.
    let resp = core.handle(audit_query("auditor", Role::Auditor, json!({})));
    assert!(resp.ok, "empty audit.query succeeds");
    assert!(rows(&resp).is_empty(), "no rows yet");

    // A filter that matches nothing is also empty (not an error).
    assert!(!core.handle(runtime_run("actor-viewer-1", Role::Viewer)).ok);
    let none = core.handle(audit_query(
        "auditor",
        Role::Auditor,
        json!({ "actor_id": "nobody" }),
    ));
    assert!(none.ok);
    assert!(rows(&none).is_empty(), "filter matched nothing");
}

#[test]
fn audit_query_is_privileged_and_a_denied_read_is_itself_audited() {
    let mut core = WorkspaceCore::in_memory("ws-audit-priv").unwrap();

    // A data-only Editor cannot read the security trail (privileged oversight read).
    let denied = core.handle(audit_query("actor-editor-1", Role::Editor, json!({})));
    assert!(!denied.ok, "an Editor cannot read the audit log");
    assert_eq!(denied.error.as_ref().unwrap().code(), "PermissionDenied");

    // The denied audit.query ITSELF landed a command-RBAC audit row, so an attempt
    // to read the security log is auditable. A privileged Auditor reads it back.
    let read = core.handle(audit_query(
        "actor-auditor-1",
        Role::Auditor,
        json!({ "action": "command.audit.query" }),
    ));
    let result = rows(&read);
    assert_eq!(
        result.len(),
        1,
        "the denied audit.query is itself a persisted command-rbac row: {result:?}"
    );
    let row = &result[0];
    assert_eq!(row["producer"], "command-rbac");
    assert_eq!(row["action"], "command.audit.query");
    assert_eq!(row["decision"], "deny");
    assert_eq!(row["actor_id"], "actor-editor-1");
    assert_eq!(row["resource_type"], "command");
    assert_eq!(row["resource_id"], "audit.query");
    assert_eq!(row["metadata"]["role"], "Editor");

    // A Viewer is likewise denied (read-only data role, not oversight).
    assert!(
        !core
            .handle(audit_query("actor-viewer-1", Role::Viewer, json!({})))
            .ok,
        "a Viewer cannot read the audit log either"
    );
    // A Runner (execution-only) is denied too.
    assert!(
        !core
            .handle(audit_query("actor-runner-1", Role::Runner, json!({})))
            .ok,
        "a Runner cannot read the audit log"
    );
}

#[test]
fn a_successful_audit_query_records_its_own_allow_row_queryable_afterward() {
    // Review 150: a SUCCESSFUL privileged read of the security trail is itself an
    // audited security event. The read records an `audit.query` ALLOW row that a
    // LATER read can find — but the recording read does NOT return its own row (the
    // result is the snapshot taken before its self-audit row lands, so there is no
    // audit-of-the-audit recursion).
    let mut core = WorkspaceCore::in_memory("ws-audit-self").unwrap();

    // A privileged Auditor reads the (empty) log, filtering for the self-audit row's
    // action. The result is empty: this very read's allow row has not landed yet.
    let first = core.handle(audit_query(
        "actor-auditor-1",
        Role::Auditor,
        json!({ "action": "audit.query" }),
    ));
    assert!(
        rows(&first).is_empty(),
        "a read does not return its own freshly-appended self-audit row"
    );

    // A SECOND privileged read now finds the FIRST read's allow row (seq 1).
    let second = core.handle(audit_query(
        "actor-maintainer-1",
        Role::Maintainer,
        json!({ "action": "audit.query" }),
    ));
    let found = rows(&second);
    assert_eq!(
        found.len(),
        1,
        "the first successful read left exactly one queryable allow row: {found:?}"
    );
    let row = &found[0];
    assert_eq!(row["producer"], "audit");
    assert_eq!(row["action"], "audit.query");
    assert_eq!(row["decision"], "allow");
    assert_eq!(row["actor_id"], "actor-auditor-1");
    assert_eq!(row["resource_type"], "audit_log");
    assert_eq!(row["resource_id"], "audit_log");
    assert_eq!(row["seq"], 1);
    // The metadata records ONLY the filter keys the query carried — never the
    // returned rows. The first read filtered by `action`, so that key (and nothing
    // resembling a returned row's contents) is present.
    assert_eq!(row["metadata"]["action"], "audit.query");
    assert!(
        row["metadata"].get("actor_id").is_none(),
        "the filter carried no actor_id predicate, so the row records none: {row:?}"
    );
    assert!(row["logical_time"].as_u64().is_some(), "logical_time present");
}

#[test]
fn a_whole_log_read_records_an_empty_filter_metadata_object() {
    // An unfiltered (whole-log) successful read still records that a read happened —
    // its self-audit allow row carries an EMPTY filter-metadata object (review 150),
    // never the contents of the rows it returned.
    let mut core = WorkspaceCore::in_memory("ws-audit-wholelog").unwrap();
    assert!(core.handle(audit_query("owner", Role::Owner, json!({}))).ok);

    let read = core.handle(audit_query(
        "owner",
        Role::Owner,
        json!({ "action": "audit.query" }),
    ));
    let found = rows(&read);
    assert_eq!(found.len(), 1, "the prior whole-log read left one row: {found:?}");
    assert_eq!(found[0]["metadata"], json!({}));
}

#[test]
fn a_denied_audit_query_records_only_its_command_rbac_deny_row() {
    // Review 150 boundary: the self-audit allow row is appended ONLY on a SUCCESSFUL
    // read. A DENIED read records its command-RBAC deny row (unchanged) and NO
    // `audit.query` allow row — a deny is never recorded as an allow.
    let mut core = WorkspaceCore::in_memory("ws-audit-deny-only").unwrap();

    // An Editor is denied the privileged read.
    let denied = core.handle(audit_query("actor-editor-1", Role::Editor, json!({})));
    assert!(!denied.ok, "an Editor cannot read the audit log");

    // Inspect the durable log directly (the raw store path appends NOTHING, unlike a
    // command `audit.query`): the denied read left EXACTLY its command-RBAC deny row
    // and NO `audit.query` allow row — a deny is never recorded as an allow.
    let all = core.store().query_audit(&AuditQuery::default()).unwrap();
    assert_eq!(
        all.len(),
        1,
        "the denied read lands exactly one row (the command-RBAC deny): {all:?}"
    );
    assert_eq!(all[0].producer, "command-rbac");
    assert_eq!(all[0].decision, "deny");
    assert_eq!(all[0].resource_type, "command");
    assert_eq!(all[0].resource_id.as_deref(), Some("audit.query"));
    let audit_allows = core
        .store()
        .query_audit(&AuditQuery {
            decision: Some("allow".into()),
            resource_type: Some("audit_log".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        audit_allows.is_empty(),
        "a denied read appends no audit.query allow row: {audit_allows:?}"
    );
}

#[test]
fn audit_query_oversight_roles_may_read() {
    let mut core = WorkspaceCore::in_memory("ws-audit-roles").unwrap();
    // Each oversight role can read the (empty) log without a PermissionDenied.
    for role in [Role::Owner, Role::Maintainer, Role::Auditor] {
        let resp = core.handle(audit_query("actor", role, json!({})));
        assert!(resp.ok, "{role:?} may read the audit log: {:?}", resp.error);
    }
}

#[test]
fn audit_query_rejects_a_malformed_filter() {
    let mut core = WorkspaceCore::in_memory("ws-audit-bad").unwrap();

    // A non-object filter is a ValidationError (never a silently-ignored filter).
    let bad_shape = core.handle(CoreCommand {
        request_id: RequestId::new("req-bad"),
        name: "audit.query".into(),
        applet_id: None::<AppletId>,
        actor: ActorContext::owner("owner"),
        workspace_id: WorkspaceId::new("ws"),
        payload: json!({ "filter": "decision=deny" }),
    });
    assert!(!bad_shape.ok);
    assert_eq!(bad_shape.error.as_ref().unwrap().code(), "ValidationError");

    // A wrong-typed predicate is rejected rather than coerced/ignored.
    let bad_type = core.handle(audit_query("owner", Role::Owner, json!({ "seq_gte": "two" })));
    assert!(!bad_type.ok);
    assert_eq!(bad_type.error.as_ref().unwrap().code(), "ValidationError");

    // A decision typo is rejected (it would otherwise silently match nothing).
    let bad_decision = core.handle(audit_query("owner", Role::Owner, json!({ "decision": "denied" })));
    assert!(!bad_decision.ok);
    assert_eq!(bad_decision.error.as_ref().unwrap().code(), "ValidationError");
}
