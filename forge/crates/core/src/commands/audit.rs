//! `audit.query` — read the durable SC-12 audit log (`forge/spec/audit-log.md` §5
//! "Query contract"). This is the privileged READ half of the audit log: the
//! producers (sync-RBAC, command-RBAC, permission, secrets, net, lifecycle,
//! signing) PERSIST rows through their live decision paths; this command is how a
//! user — or an auditor — reads them back to answer "who did what, and was it
//! allowed?" after the fact.
//!
//! The persisted rows are ALREADY redacted at append time (`redact_metadata` is the
//! single chokepoint, applied on every `append_audit_tx`), so this read path never
//! re-redacts — it cannot surface a secret value or a request/response body because
//! none was ever written. What it adds over the raw `Store::query_audit` is the
//! COMMAND boundary: the same CR-A3 role gate every command passes, scoped so that
//! reading the security trail is a privileged operation (audit read = the oversight
//! roles, mirroring `runtime.replay`). A role that may not read the log is denied
//! BEFORE any row is read, and that denial itself lands a command-RBAC audit row
//! through the live `WorkspaceCore::handle` path — so an attempt to read the audit
//! log is itself auditable.

use forge_domain::{CoreError, Result};
use forge_storage::AuditQuery;

use super::super::WorkspaceCore;

impl WorkspaceCore {
    /// `audit.query` — return the durable audit-log rows matching the payload filter
    /// (SC-12, `forge/spec/audit-log.md` §5), ordered by `seq` ascending (the
    /// deterministic ordering key). Payload `{ filter? }` where `filter` AND-combines
    /// any of:
    ///
    ///   - exact `actor_id`, `action`, `decision`, `resource_type`, `resource_id`,
    ///     `collection`;
    ///   - inclusive `seq` range (`seq_gte` / `seq_lte`);
    ///   - inclusive `logical_time` range (`logical_time_gte` / `logical_time_lte`).
    ///
    /// An absent / empty `filter` returns every row; a filter that matches nothing
    /// returns an empty `rows` array (the empty-result path is part of the contract,
    /// not an error). Each returned row is the full redacted [`AuditRecord`] shape
    /// (`audit_id`, `seq`, `logical_time`, `producer`, `action`, `decision`,
    /// `actor_id`, `resource_type`, `resource_id`, `collection`, `reason`,
    /// `metadata`).
    ///
    /// Authorization: the command-level [`authorize`](super::super::auth::authorize)
    /// role gate (run BEFORE dispatch in `WorkspaceCore::handle`) restricts this to
    /// the oversight roles (Owner, Maintainer, Auditor) — reading the security trail
    /// is privileged, exactly like `runtime.replay`. A role-denied `audit.query`
    /// never reaches this handler; the denial lands a command-RBAC audit row through
    /// the live path. The rows are already redacted at persistence, so this read
    /// surface adds no further redaction obligation.
    pub(in crate::workspace) fn cmd_audit_query(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let filter = audit_filter_from_payload(cmd)?;
        let rows = self.store.query_audit(&filter)?;
        let rows: Vec<serde_json::Value> = rows.iter().map(audit_row_to_json).collect();
        Ok(serde_json::json!({ "rows": rows }))
    }
}

/// Build an [`AuditQuery`] from the command's `payload.filter` object (SC-12 query
/// surface). Each predicate is OPTIONAL; an absent `filter` (or an absent key within
/// it) leaves that predicate `None`, so a payload-less `audit.query` returns the
/// whole log. A present-but-wrong-TYPE predicate is a `ValidationError` rather than a
/// silently-ignored filter, so a malformed query is surfaced instead of widening the
/// result.
fn audit_filter_from_payload(cmd: &forge_domain::CoreCommand) -> Result<AuditQuery> {
    let filter = match cmd.payload.get("filter") {
        None | Some(serde_json::Value::Null) => return Ok(AuditQuery::default()),
        Some(serde_json::Value::Object(map)) => map,
        Some(other) => {
            return Err(CoreError::ValidationError(format!(
                "audit.query `filter` must be an object, got {other}"
            )))
        }
    };

    Ok(AuditQuery {
        actor_id: string_field(filter, "actor_id")?,
        action: string_field(filter, "action")?,
        decision: decision_field(filter)?,
        resource_type: string_field(filter, "resource_type")?,
        resource_id: string_field(filter, "resource_id")?,
        collection: string_field(filter, "collection")?,
        seq_gte: u64_field(filter, "seq_gte")?,
        seq_lte: u64_field(filter, "seq_lte")?,
        logical_time_gte: u64_field(filter, "logical_time_gte")?,
        logical_time_lte: u64_field(filter, "logical_time_lte")?,
    })
}

/// Read an optional string predicate. Absent ⇒ `None`; present-but-non-string is a
/// `ValidationError` (a typed filter never coerces).
fn string_field(
    filter: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>> {
    match filter.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(CoreError::ValidationError(format!(
            "audit.query filter `{key}` must be a string, got {other}"
        ))),
    }
}

/// Read the optional `decision` predicate, validating it is `allow` / `deny` — a
/// typo (`denied`) would otherwise silently match nothing, so it is a typed error.
fn decision_field(
    filter: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>> {
    match string_field(filter, "decision")? {
        None => Ok(None),
        Some(d) if d == "allow" || d == "deny" => Ok(Some(d)),
        Some(other) => Err(CoreError::ValidationError(format!(
            "audit.query filter `decision` must be \"allow\" or \"deny\", got {other:?}"
        ))),
    }
}

/// Read an optional `u64` range bound. Absent ⇒ `None`; a present non-integer (or a
/// negative number) is a `ValidationError` so a malformed range never widens the
/// query.
fn u64_field(
    filter: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>> {
    match filter.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            CoreError::ValidationError(format!(
                "audit.query filter `{key}` must be a non-negative integer, got {v}"
            ))
        }),
    }
}

/// Serialize one persisted [`AuditRecord`] to the wire row shape (the manifest
/// `row_shape`). `metadata` is already redacted at persistence, so it is echoed
/// verbatim; nullable `resource_id`/`collection` serialize as JSON `null` when
/// absent.
fn audit_row_to_json(row: &forge_storage::AuditRecord) -> serde_json::Value {
    serde_json::json!({
        "audit_id": row.audit_id,
        "seq": row.seq,
        "logical_time": row.logical_time,
        "producer": row.producer,
        "action": row.action,
        "decision": row.decision,
        "actor_id": row.actor_id,
        "resource_type": row.resource_type,
        "resource_id": row.resource_id,
        "collection": row.collection,
        "reason": row.reason,
        "metadata": row.metadata,
    })
}
