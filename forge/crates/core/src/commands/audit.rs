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
//!
//! Both OUTCOMES of an `audit.query` are therefore traceable (review 150): a denied
//! read lands the command-RBAC deny row (above), and a SUCCESSFUL read lands its own
//! `audit.query` ALLOW row (`producer = audit`, `resource_type/_id = audit_log`)
//! through the live producer seam. That self-audit row records ONLY the filter
//! keys/ranges the query carried — never the returned row contents — so reading the
//! log can never leak a persisted actor/resource via the row that records the read,
//! and it cannot recurse (the append writes one row directly, never re-entering
//! `cmd_audit_query`).

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
        // Read the matching rows FIRST, then append the `audit.query` allow row. The
        // returned `rows` are the snapshot taken BEFORE this read's own audit row
        // lands, so a successful read never returns its own freshly-appended
        // self-audit row — the read of the audit log is recorded WITHOUT the row
        // becoming part of the result it is recording (review 150 recursion guard).
        let rows = self.store.query_audit(&filter)?;
        let json_rows: Vec<serde_json::Value> = rows.iter().map(audit_row_to_json).collect();

        // SC-12 review 150: a SUCCESSFUL privileged read of the security trail is
        // itself a security event — reading "who did what" is privileged (the role
        // gate already restricted it to the oversight roles), so the read must be
        // traceable like every other audited decision. Persist an `audit.query`
        // ALLOW row through the live producer seam (`persist_producer_audit` →
        // `append_audit` → `redact_metadata`). The metadata is built from ONLY the
        // filter keys/ranges this query carried — NEVER the returned row contents —
        // so reading the log can never leak a persisted actor/resource through the
        // self-audit row, and the row stays bounded regardless of result size.
        //
        // No code-level recursion: this append writes one row directly to the store;
        // it does NOT re-enter `WorkspaceCore::handle` or `cmd_audit_query`, so there
        // is no audit-of-the-audit loop.
        //
        // Review 156: the self-audit append is REQUIRED, not best-effort. The read
        // already succeeded (`rows` are in hand), so propagating the append failure
        // via `?` either records the read or FAILS the `audit.query` — it never
        // returns rows whose access went unlogged. A discarded `let _ =` would fail
        // OPEN: a privileged read of the oversight-only security trail could complete
        // and return rows while its own audit row silently failed to persist, exactly
        // the no-unlogged-privileged-read invariant review 150 set out to guarantee.
        // The append can only ADD an allow row; it never suppresses a deny or alters
        // the returned result.
        let metadata = filter_metadata(&filter);
        let actor_id = cmd.actor.actor.as_str().to_string();
        // TEST-ONLY hook (review 156, gated by `test-hooks` so it is unreachable from
        // an untrusted payload — see `commands::test_hooks`): inject a self-audit
        // append failure AFTER the rows were read, proving the `?` below fails the
        // `audit.query` (returns NO rows) rather than returning the already-read rows
        // with their access silently unlogged. Mirrors a real `append_audit` SQL /
        // serialize error.
        if super::test_hooks::simulate_failure_at(cmd, "self_audit_append") {
            return Err(CoreError::StorageError(
                "simulated audit.query self-audit append failure".into(),
            ));
        }
        self.persist_producer_audit(
            "audit.query",
            serde_json::json!({
                "decision": "allow",
                "actor_id": actor_id,
                "action": "audit.query",
            }),
            "audit",
            "audit.query",
            "allow",
            actor_id,
            "audit_log",
            Some("audit_log".to_string()),
            None,
            "audit log read authorized for oversight role",
            metadata,
        )?;

        Ok(serde_json::json!({ "rows": json_rows }))
    }
}

/// Build the `audit.query` allow row's `metadata` from ONLY the query's filter
/// predicates (review 150) — the set exact-match keys and the inclusive `seq` /
/// `logical_time` range bounds. This deliberately records WHAT was queried, never
/// the rows that were RETURNED: echoing result contents into the self-audit row
/// would leak persisted actor/resource ids and grow unbounded with the result set.
/// An all-`None` filter (a whole-log read) yields an empty object, which still
/// records that an unfiltered read happened.
fn filter_metadata(filter: &AuditQuery) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut put_str = |key: &str, value: &Option<String>| {
        if let Some(v) = value {
            map.insert(key.to_string(), serde_json::json!(v));
        }
    };
    put_str("actor_id", &filter.actor_id);
    put_str("action", &filter.action);
    put_str("decision", &filter.decision);
    put_str("resource_type", &filter.resource_type);
    put_str("resource_id", &filter.resource_id);
    put_str("collection", &filter.collection);
    let mut put_u64 = |key: &str, value: Option<u64>| {
        if let Some(v) = value {
            map.insert(key.to_string(), serde_json::json!(v));
        }
    };
    put_u64("seq_gte", filter.seq_gte);
    put_u64("seq_lte", filter.seq_lte);
    put_u64("logical_time_gte", filter.logical_time_gte);
    put_u64("logical_time_lte", filter.logical_time_lte);
    serde_json::Value::Object(map)
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
