//! SC-12 **durable append-only audit log**: the persisted, queryable record of
//! every security-relevant authorization decision the real producers emit
//! (sync-RBAC, command-RBAC, permission grants/revokes, secret use, network
//! egress, applet lifecycle purge, signed-install refusal).
//!
//! `forge/spec/audit-log.md` is the normative contract; the
//! `forge/fixtures/audit-log-e2e` vectors are the behavioral one. The invariants
//! this module enforces:
//!
//! - **Append-only** — there is NO update/delete path for a prior row. Re-running
//!   a producer [`append_audit_tx`]s a NEW row with a fresh `seq`/`audit_id`; it
//!   never mutates history.
//! - **Deterministic ordering** — `seq` is a workspace-local monotonic counter
//!   minted from a persisted KV counter, so the ordering key replays identically.
//!   `logical_time` is supplied by the CALLER (the EventSink logical clock or an
//!   externally supplied replay clock) — this module NEVER calls a wall clock on
//!   the persisted-row path, so a recorded run reproduces byte-identical rows.
//! - **Redaction** — [`redact_metadata`] is applied before persistence: a secret
//!   audit row carries only the `secret_ref` id, never the resolved value; a
//!   network row carries method/host/path/status, never request or response
//!   bodies. The redaction helper drops the sensitive keys and stamps a
//!   `*_redacted: true` marker so the row records that a value was withheld.
//!
//! The append happens inside the CALLER's [`Store::transact`] (via
//! [`append_audit_tx`]) so a decision and its audit row commit — or roll back —
//! coherently: a denied op that rolls back never leaves an orphan audit row, and
//! a committed decision always lands its row.

use forge_domain::{CoreError, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{Map, Value};

use crate::errors::{map_json, map_sql};
use crate::store::Store;

/// The KV namespace holding workspace metadata (mirrors the migration driver's
/// `META_NS` and the core's `__forge/meta`). The audit `seq` counter lives here
/// so it survives reopen and is the single workspace-local monotonic anchor.
const AUDIT_META_NS: &str = "__forge/meta";

/// The KV key (within [`AUDIT_META_NS`]) holding the highest assigned audit
/// `seq` as utf-8 decimal text. Absent → no rows yet; the first assigned seq is
/// `1` unless a caller pins a starting `next_seq` via [`Store::set_audit_seq`].
const AUDIT_SEQ_KEY: &str = "audit_seq";

/// Canonical metadata key marker stamped when a secret VALUE was withheld from a
/// persisted audit row (`forge/spec/audit-log.md` §Redaction). The row keeps the
/// `secret_ref` id; the resolved secret never persists.
const VALUE_REDACTED_MARKER: &str = "value_redacted";

/// Metadata keys whose values are SECRET material and must never persist. The
/// redaction helper drops them and (for `secret`/`value`) stamps
/// [`VALUE_REDACTED_MARKER`].
const SECRET_VALUE_KEYS: &[&str] = &["secret_value", "value", "resolved_secret", "secret"];

/// Metadata keys carrying request/response BODIES that must never persist
/// (`forge/spec/audit-log.md` §Redaction). The redaction helper drops each and
/// stamps the matching `*_redacted: true` marker so the row records the omission.
const BODY_KEYS: &[(&str, &str)] = &[
    ("request_body", "request_body_redacted"),
    ("response_body", "response_body_redacted"),
    ("body", "request_body_redacted"),
];

/// One persisted audit row (SC-12). Mirrors the manifest `row_shape`: a stable
/// `audit_id` minted from `seq`, the workspace-local monotonic `seq`, the
/// caller-supplied deterministic `logical_time`, the producing subsystem, the
/// canonical action, the allow/deny decision, the responsible actor, the
/// resource it touched, an optional collection, the decisive reason, and the
/// REDACTED structured `metadata` (never a secret value or a request/response
/// body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Stable row id minted from `seq` as `audit-{seq:06}` (set by the store on
    /// append; ignored on the input record).
    pub audit_id: String,
    /// Workspace-local monotonic append sequence (assigned by the store).
    pub seq: u64,
    /// Deterministic logical timestamp supplied by the caller from the EventSink
    /// logical clock or an externally supplied replay clock — NEVER a wall clock.
    pub logical_time: u64,
    /// Subsystem that emitted the row (e.g. `sync-rbac`, `command-rbac`,
    /// `permission-manager`, `secrets`, `net`, `lifecycle`, `signing`).
    pub producer: String,
    /// Canonical action string (e.g. `sync.record.insert`, `command.runtime.run`,
    /// `permission.grant`, `secret.use`, `network.egress`, `applet.uninstalled`,
    /// `package.install.refused`).
    pub action: String,
    /// `"allow"` or `"deny"`.
    pub decision: String,
    /// Authenticated actor responsible for the decision.
    pub actor_id: String,
    /// Resource kind: `record`, `schema`, `command`, `capability`, `secret`,
    /// `network`, `applet`, `package`, or `audit_log`.
    pub resource_type: String,
    /// Stable resource id when present (nullable).
    pub resource_id: Option<String>,
    /// Record collection when present (nullable).
    pub collection: Option<String>,
    /// Human-readable decisive check.
    pub reason: String,
    /// Redacted structured context — never a secret value, request body, or
    /// response body. [`redact_metadata`] is applied on append.
    pub metadata: Value,
}

impl AuditRecord {
    /// Build an audit record to append. `audit_id`/`seq` are placeholders the
    /// store overwrites on [`append_audit_tx`]; `logical_time` is the caller's
    /// deterministic clock value.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        logical_time: u64,
        producer: impl Into<String>,
        action: impl Into<String>,
        decision: impl Into<String>,
        actor_id: impl Into<String>,
        resource_type: impl Into<String>,
        resource_id: Option<String>,
        collection: Option<String>,
        reason: impl Into<String>,
        metadata: Value,
    ) -> Self {
        AuditRecord {
            audit_id: String::new(),
            seq: 0,
            logical_time,
            producer: producer.into(),
            action: action.into(),
            decision: decision.into(),
            actor_id: actor_id.into(),
            resource_type: resource_type.into(),
            resource_id,
            collection,
            reason: reason.into(),
            metadata,
        }
    }
}

/// The stable `audit_id` minted from a `seq` (`audit-{seq:06}`). Padding to six
/// digits matches the fixtures; a seq beyond six digits widens naturally.
pub fn audit_id_for_seq(seq: u64) -> String {
    format!("audit-{seq:06}")
}

/// A filter over the audit log ([`Store::query_audit`]). All set fields are
/// AND-combined; an all-`None` filter matches every row. Results are always
/// ordered by `seq` ascending (the deterministic ordering key,
/// `forge/spec/audit-log.md` §Query).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditQuery {
    /// Exact actor id.
    pub actor_id: Option<String>,
    /// Exact canonical action.
    pub action: Option<String>,
    /// `allow` / `deny`.
    pub decision: Option<String>,
    /// Exact resource type.
    pub resource_type: Option<String>,
    /// Exact resource id.
    pub resource_id: Option<String>,
    /// Exact collection.
    pub collection: Option<String>,
    /// Inclusive lower bound on `seq`.
    pub seq_gte: Option<u64>,
    /// Inclusive upper bound on `seq`.
    pub seq_lte: Option<u64>,
    /// Inclusive lower bound on `logical_time`.
    pub logical_time_gte: Option<u64>,
    /// Inclusive upper bound on `logical_time`.
    pub logical_time_lte: Option<u64>,
}

impl AuditQuery {
    /// Filter by exact actor id.
    pub fn by_actor(actor_id: impl Into<String>) -> Self {
        AuditQuery {
            actor_id: Some(actor_id.into()),
            ..Default::default()
        }
    }

    /// Filter by exact action.
    pub fn by_action(action: impl Into<String>) -> Self {
        AuditQuery {
            action: Some(action.into()),
            ..Default::default()
        }
    }

    /// Filter by decision (`allow`/`deny`).
    pub fn by_decision(decision: impl Into<String>) -> Self {
        AuditQuery {
            decision: Some(decision.into()),
            ..Default::default()
        }
    }

    /// Filter by exact resource type.
    pub fn by_resource_type(resource_type: impl Into<String>) -> Self {
        AuditQuery {
            resource_type: Some(resource_type.into()),
            ..Default::default()
        }
    }

    /// Filter by exact resource id.
    pub fn by_resource_id(resource_id: impl Into<String>) -> Self {
        AuditQuery {
            resource_id: Some(resource_id.into()),
            ..Default::default()
        }
    }

    /// Filter by exact collection.
    pub fn by_collection(collection: impl Into<String>) -> Self {
        AuditQuery {
            collection: Some(collection.into()),
            ..Default::default()
        }
    }

    /// Filter by an inclusive `seq` range.
    pub fn seq_range(gte: u64, lte: u64) -> Self {
        AuditQuery {
            seq_gte: Some(gte),
            seq_lte: Some(lte),
            ..Default::default()
        }
    }
}

impl Store {
    /// Pin the next audit `seq` to assign (the value `next_seq` will be the FIRST
    /// seq the next [`append_audit_tx`] hands out). Used by the fixture harness to
    /// seed the deterministic starting sequence each vector pins; the counter is
    /// stored as `next_seq - 1` (the highest "assigned" seq) so the next append
    /// returns `next_seq`.
    pub fn set_audit_seq(&mut self, next_seq: u64) -> Result<()> {
        let stored = next_seq.saturating_sub(1);
        self.transact(|tx| {
            crate::kv::kv_set_tx(
                tx,
                AUDIT_META_NS,
                AUDIT_SEQ_KEY,
                stored.to_string().as_bytes(),
                "text/plain",
            )
        })
    }

    /// The highest audit `seq` assigned so far (0 when the log is empty). The next
    /// [`append_audit_tx`] assigns `highest_audit_seq() + 1`.
    pub fn highest_audit_seq(&self) -> Result<u64> {
        match self.kv_get(AUDIT_META_NS, AUDIT_SEQ_KEY)? {
            Some(bytes) => crate::errors::parse_counter_value(&bytes),
            None => Ok(0),
        }
    }

    /// Append one audit row inside the CALLER's open transaction (SC-12), so the
    /// decision and its audit row commit — or roll back — together. Assigns the
    /// next monotonic `seq` from the persisted counter, mints `audit_id` from it,
    /// REDACTS `metadata` (no secret value / request / response body), and inserts
    /// the row. There is NO update/delete path: this only ever APPENDS.
    ///
    /// `logical_time` is taken verbatim from the input record (the caller's
    /// deterministic EventSink/replay clock); this method never reads a wall
    /// clock, so a replayed run reproduces byte-identical rows.
    ///
    /// Returns the persisted row (with the assigned `seq`/`audit_id` and the
    /// redacted metadata) so the caller can echo or assert it.
    pub fn append_audit_tx(
        tx: &rusqlite::Transaction<'_>,
        record: &AuditRecord,
    ) -> Result<AuditRecord> {
        let next_seq = next_audit_seq_tx(tx)?;
        let audit_id = audit_id_for_seq(next_seq);
        let metadata = redact_metadata(&record.metadata);
        let metadata_json =
            serde_json::to_string(&metadata).map_err(|e| map_json("append_audit_tx", e))?;
        tx.execute(
            "INSERT INTO audit_log
                 (seq, audit_id, logical_time, producer, action, decision,
                  actor_id, resource_type, resource_id, collection, reason, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                next_seq as i64,
                audit_id,
                record.logical_time as i64,
                record.producer,
                record.action,
                record.decision,
                record.actor_id,
                record.resource_type,
                record.resource_id,
                record.collection,
                record.reason,
                metadata_json,
            ],
        )
        .map_err(map_sql)?;
        Ok(AuditRecord {
            audit_id,
            seq: next_seq,
            metadata,
            ..record.clone()
        })
    }

    /// Append one audit row in its own single transaction (the stand-alone form of
    /// [`append_audit_tx`], for callers that are not already composing a larger
    /// atomic write). Most security producers should prefer the `_tx` form so the
    /// decision and its row commit coherently.
    pub fn append_audit(&mut self, record: &AuditRecord) -> Result<AuditRecord> {
        self.transact(|tx| Store::append_audit_tx(tx, record))
    }

    /// Query the audit log (SC-12), filtered by [`AuditQuery`] and ordered by
    /// `seq` ascending (the deterministic ordering key). An all-`None` filter
    /// returns every row; a filter that matches nothing returns an empty `Vec`.
    pub fn query_audit(&self, filter: &AuditQuery) -> Result<Vec<AuditRecord>> {
        let mut sql = String::from(
            "SELECT seq, audit_id, logical_time, producer, action, decision,
                    actor_id, resource_type, resource_id, collection, reason, metadata
               FROM audit_log",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let push_eq = |col: &str, val: &Option<String>, binds: &mut Vec<Box<dyn rusqlite::ToSql>>, clauses: &mut Vec<String>| {
            if let Some(v) = val {
                clauses.push(format!("{col} = ?{}", binds.len() + 1));
                binds.push(Box::new(v.clone()));
            }
        };
        push_eq("actor_id", &filter.actor_id, &mut binds, &mut clauses);
        push_eq("action", &filter.action, &mut binds, &mut clauses);
        push_eq("decision", &filter.decision, &mut binds, &mut clauses);
        push_eq("resource_type", &filter.resource_type, &mut binds, &mut clauses);
        push_eq("resource_id", &filter.resource_id, &mut binds, &mut clauses);
        push_eq("collection", &filter.collection, &mut binds, &mut clauses);
        let push_u64 = |col: &str, op: &str, val: Option<u64>, binds: &mut Vec<Box<dyn rusqlite::ToSql>>, clauses: &mut Vec<String>| {
            if let Some(v) = val {
                clauses.push(format!("{col} {op} ?{}", binds.len() + 1));
                binds.push(Box::new(v as i64));
            }
        };
        push_u64("seq", ">=", filter.seq_gte, &mut binds, &mut clauses);
        push_u64("seq", "<=", filter.seq_lte, &mut binds, &mut clauses);
        push_u64("logical_time", ">=", filter.logical_time_gte, &mut binds, &mut clauses);
        push_u64("logical_time", "<=", filter.logical_time_lte, &mut binds, &mut clauses);
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY seq ASC");

        let mut stmt = self.conn.prepare(&sql).map_err(map_sql)?;
        let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bind_refs), row_to_audit)
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)??);
        }
        Ok(out)
    }
}

/// Reserve the next monotonic audit `seq` inside the open transaction by reading
/// the persisted counter, bumping it, and writing it back. The whole read-bump-
/// write runs in the caller's transaction, so two appends in one transaction get
/// consecutive seqs and a rollback un-reserves them (the counter rolls back with
/// the rows — the log is gap-free per committed run).
fn next_audit_seq_tx(tx: &rusqlite::Transaction<'_>) -> Result<u64> {
    let current: u64 = tx
        .query_row(
            "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2 AND tombstone = 0",
            params![AUDIT_META_NS, AUDIT_SEQ_KEY],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten()
        .map(|bytes| crate::errors::parse_counter_value(&bytes))
        .transpose()?
        .unwrap_or(0);
    let next = current
        .checked_add(1)
        .ok_or_else(|| CoreError::StorageError("audit seq overflowed u64".into()))?;
    crate::kv::kv_set_tx(
        tx,
        AUDIT_META_NS,
        AUDIT_SEQ_KEY,
        next.to_string().as_bytes(),
        "text/plain",
    )?;
    Ok(next)
}

/// Map one `audit_log` row to an [`AuditRecord`], rehydrating the metadata JSON.
/// The metadata column is canonical JSON; a corrupt value surfaces a
/// `StorageError` rather than silently dropping context.
fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<AuditRecord>> {
    let metadata_str: String = row.get(11)?;
    let seq = row.get::<_, i64>(0)? as u64;
    let logical_time = row.get::<_, i64>(2)? as u64;
    let producer: String = row.get(3)?;
    let action: String = row.get(4)?;
    let decision: String = row.get(5)?;
    let actor_id: String = row.get(6)?;
    let resource_type: String = row.get(7)?;
    let resource_id: Option<String> = row.get(8)?;
    let collection: Option<String> = row.get(9)?;
    let reason: String = row.get(10)?;
    let audit_id: String = row.get(1)?;
    Ok((|| {
        let metadata: Value =
            serde_json::from_str(&metadata_str).map_err(|e| map_json("query_audit", e))?;
        Ok(AuditRecord {
            audit_id,
            seq,
            logical_time,
            producer,
            action,
            decision,
            actor_id,
            resource_type,
            resource_id,
            collection,
            reason,
            metadata,
        })
    })())
}

/// Redact a metadata object before persistence (`forge/spec/audit-log.md`
/// §Redaction): drop any SECRET-value key (replacing it with a
/// `value_redacted: true` marker) and any request/response BODY key (replacing it
/// with the matching `*_redacted: true` marker). A `secret_ref` id is NOT
/// secret material and is preserved. Non-object metadata is returned unchanged
/// (producers persist objects; a scalar/array carries no keyed secret to drop).
///
/// This is intentionally a pure value→value transform so it can be unit-tested in
/// isolation and applied uniformly on every append, regardless of producer.
pub fn redact_metadata(metadata: &Value) -> Value {
    let Value::Object(obj) = metadata else {
        return metadata.clone();
    };
    let mut out = Map::with_capacity(obj.len());
    let mut secret_dropped = false;
    let mut request_body_dropped = false;
    let mut response_body_dropped = false;
    for (key, value) in obj {
        if SECRET_VALUE_KEYS.contains(&key.as_str()) {
            secret_dropped = true;
            continue;
        }
        if let Some((_, marker)) = BODY_KEYS.iter().find(|(k, _)| *k == key.as_str()) {
            match *marker {
                "request_body_redacted" => request_body_dropped = true,
                "response_body_redacted" => response_body_dropped = true,
                _ => {}
            }
            continue;
        }
        // Recurse into nested objects so a body/secret nested under `request` or
        // `response` (e.g. `{"request": {"body": ...}}`) is redacted too.
        out.insert(key.clone(), redact_metadata(value));
    }
    if secret_dropped {
        out.entry(VALUE_REDACTED_MARKER.to_string())
            .or_insert(Value::Bool(true));
    }
    if request_body_dropped {
        out.entry("request_body_redacted".to_string())
            .or_insert(Value::Bool(true));
    }
    if response_body_dropped {
        out.entry("response_body_redacted".to_string())
            .or_insert(Value::Bool(true));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    /// A minimal deny record for the command-RBAC producer at the given logical
    /// time (the metadata is already body/secret-free).
    fn deny_record(seq_logical_time: u64, actor: &str) -> AuditRecord {
        AuditRecord::new(
            seq_logical_time,
            "command-rbac",
            "command.runtime.run",
            "deny",
            actor,
            "command",
            Some("runtime.run".to_string()),
            None,
            "auditor is not permitted to run applet code",
            json!({"role": "Auditor", "command": "runtime.run"}),
        )
    }

    #[test]
    fn append_assigns_monotonic_seq_and_audit_id() {
        let mut s = store();
        let r1 = s.append_audit(&deny_record(1000, "actor-a")).unwrap();
        let r2 = s.append_audit(&deny_record(1001, "actor-b")).unwrap();
        let r3 = s.append_audit(&deny_record(1002, "actor-c")).unwrap();
        assert_eq!((r1.seq, r2.seq, r3.seq), (1, 2, 3), "seq is monotonic from 1");
        assert_eq!(r1.audit_id, "audit-000001");
        assert_eq!(r2.audit_id, "audit-000002");
        assert_eq!(r3.audit_id, "audit-000003");
        // logical_time is taken verbatim from the caller (no wall clock).
        assert_eq!((r1.logical_time, r2.logical_time, r3.logical_time), (1000, 1001, 1002));
    }

    #[test]
    fn set_audit_seq_pins_the_starting_sequence() {
        // The fixture harness pins next_seq per vector; the first append then
        // assigns exactly that seq (mirrors the manifest's `next_seq` field).
        let mut s = store();
        s.set_audit_seq(10).unwrap();
        let r = s.append_audit(&deny_record(2001, "actor-auditor-1")).unwrap();
        assert_eq!(r.seq, 10);
        assert_eq!(r.audit_id, "audit-000010");
        let r2 = s.append_audit(&deny_record(2002, "actor-auditor-1")).unwrap();
        assert_eq!(r2.seq, 11);
    }

    #[test]
    fn two_appends_in_one_transaction_get_consecutive_seqs() {
        // The permission grant+revoke vector appends two rows in one decision;
        // they must take consecutive seqs (20, 21), not collide on one.
        let mut s = store();
        s.set_audit_seq(20).unwrap();
        let (g, r) = s
            .transact(|tx| {
                let grant = AuditRecord::new(
                    3001,
                    "permission-manager",
                    "permission.grant",
                    "allow",
                    "actor-owner-1",
                    "capability",
                    Some("db.write:collection:tasks".to_string()),
                    Some("tasks".to_string()),
                    "owner approved capability grant",
                    json!({"namespace": "db"}),
                );
                let revoke = AuditRecord::new(
                    3002,
                    "permission-manager",
                    "permission.revoke",
                    "allow",
                    "actor-owner-1",
                    "capability",
                    Some("db.write:collection:tasks".to_string()),
                    Some("tasks".to_string()),
                    "owner revoked capability grant",
                    json!({"namespace": "db"}),
                );
                let g = Store::append_audit_tx(tx, &grant)?;
                let r = Store::append_audit_tx(tx, &revoke)?;
                Ok((g, r))
            })
            .unwrap();
        assert_eq!((g.seq, r.seq), (20, 21));
        assert_eq!(g.audit_id, "audit-000020");
        assert_eq!(r.audit_id, "audit-000021");
    }

    #[test]
    fn query_filters_by_actor_action_decision_resource_collection_and_range() {
        let mut s = store();
        // seq 1: permission grant (owner, capability, tasks, allow)
        s.append_audit(&AuditRecord::new(
            10,
            "permission-manager",
            "permission.grant",
            "allow",
            "actor-owner-1",
            "capability",
            Some("db.write:collection:tasks".to_string()),
            Some("tasks".to_string()),
            "owner approved capability grant",
            json!({}),
        ))
        .unwrap();
        // seq 2: network egress (runner, network, allow)
        s.append_audit(&AuditRecord::new(
            11,
            "net",
            "network.egress",
            "allow",
            "actor-runner-1",
            "network",
            Some("https://api.example.com".to_string()),
            None,
            "network policy allowed request",
            json!({"host": "api.example.com"}),
        ))
        .unwrap();
        // seq 3: permission revoke (owner, capability, tasks, allow)
        s.append_audit(&AuditRecord::new(
            12,
            "permission-manager",
            "permission.revoke",
            "allow",
            "actor-owner-1",
            "capability",
            Some("db.write:collection:tasks".to_string()),
            Some("tasks".to_string()),
            "owner revoked capability grant",
            json!({}),
        ))
        .unwrap();
        // seq 4: sync deny (viewer, record, tasks, deny)
        s.append_audit(&AuditRecord::new(
            13,
            "sync-rbac",
            "sync.record.insert",
            "deny",
            "actor-viewer-1",
            "record",
            Some("tasks".to_string()),
            Some("tasks".to_string()),
            "viewer cannot author remote record writes",
            json!({}),
        ))
        .unwrap();

        let ids = |rows: Vec<AuditRecord>| -> Vec<String> {
            rows.into_iter().map(|r| r.audit_id).collect()
        };

        // by actor
        assert_eq!(
            ids(s.query_audit(&AuditQuery::by_actor("actor-owner-1")).unwrap()),
            vec!["audit-000001", "audit-000003"]
        );
        // by action
        assert_eq!(
            ids(s.query_audit(&AuditQuery::by_action("permission.revoke")).unwrap()),
            vec!["audit-000003"]
        );
        // by decision
        assert_eq!(
            ids(s.query_audit(&AuditQuery::by_decision("deny")).unwrap()),
            vec!["audit-000004"]
        );
        // by resource type
        assert_eq!(
            ids(s.query_audit(&AuditQuery::by_resource_type("network")).unwrap()),
            vec!["audit-000002"]
        );
        // by resource id
        assert_eq!(
            ids(s
                .query_audit(&AuditQuery::by_resource_id("db.write:collection:tasks"))
                .unwrap()),
            vec!["audit-000001", "audit-000003"]
        );
        // by collection
        assert_eq!(
            ids(s.query_audit(&AuditQuery::by_collection("tasks")).unwrap()),
            vec!["audit-000001", "audit-000003", "audit-000004"]
        );
        // by seq range (inclusive 2..=3)
        assert_eq!(
            ids(s.query_audit(&AuditQuery::seq_range(2, 3)).unwrap()),
            vec!["audit-000002", "audit-000003"]
        );
        // all rows ordered by seq ascending
        assert_eq!(
            ids(s.query_audit(&AuditQuery::default()).unwrap()),
            vec!["audit-000001", "audit-000002", "audit-000003", "audit-000004"]
        );
    }

    #[test]
    fn query_empty_result_path() {
        let mut s = store();
        s.append_audit(&deny_record(1, "actor-a")).unwrap();
        // No actor matches → empty, not an error.
        let rows = s.query_audit(&AuditQuery::by_actor("nobody")).unwrap();
        assert!(rows.is_empty());
        // And an empty log queries empty too.
        let empty = store();
        assert!(empty.query_audit(&AuditQuery::default()).unwrap().is_empty());
    }

    #[test]
    fn append_only_rerun_adds_rows_never_mutates_prior() {
        let mut s = store();
        let first = s.append_audit(&deny_record(9001, "actor-auditor-1")).unwrap();
        // Re-run the SAME producer operation: it appends a NEW row (new seq +
        // audit_id), never rewrites the prior one.
        let second = s.append_audit(&deny_record(9002, "actor-auditor-1")).unwrap();
        assert_ne!(first.seq, second.seq);
        assert_ne!(first.audit_id, second.audit_id);

        let rows = s
            .query_audit(&AuditQuery::by_actor("actor-auditor-1"))
            .unwrap();
        assert_eq!(rows.len(), 2, "both runs persisted; history grew, not mutated");
        // The first row is byte-identical to what was appended — untouched.
        assert_eq!(rows[0].seq, first.seq);
        assert_eq!(rows[0].audit_id, first.audit_id);
        assert_eq!(rows[0].logical_time, 9001);
        assert_eq!(rows[0].reason, first.reason);
        // There is no UPDATE/DELETE path: assert the table only ever grows.
        let count: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn redaction_never_persists_secret_value() {
        let mut s = store();
        // A producer hands metadata that ACCIDENTALLY contains the resolved secret
        // value alongside the secret_ref. The persisted row must keep only the ref.
        let rec = AuditRecord::new(
            4001,
            "secrets",
            "secret.use",
            "allow",
            "actor-runner-1",
            "secret",
            Some("secret_weather".to_string()),
            None,
            "secret_ref injected into allowlisted header",
            json!({
                "secret_ref": "secret_weather",
                "secret_value": "Bearer abc123",
                "value": "abc123",
                "target_host": "api.weather.example"
            }),
        );
        let persisted = s.append_audit(&rec).unwrap();
        // The returned record's metadata is already redacted.
        let meta = persisted.metadata.as_object().unwrap();
        assert_eq!(meta.get("secret_ref").unwrap(), "secret_weather");
        assert!(!meta.contains_key("secret_value"), "secret value dropped");
        assert!(!meta.contains_key("value"), "raw value dropped");
        assert_eq!(meta.get("value_redacted").unwrap(), &Value::Bool(true));
        assert_eq!(meta.get("target_host").unwrap(), "api.weather.example");

        // And the raw bytes in the DB never contain the secret.
        let raw: String = s
            .conn
            .query_row("SELECT metadata FROM audit_log WHERE seq = ?1", [persisted.seq as i64], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains("Bearer abc123"), "stored row leaks secret: {raw}");
        assert!(!raw.contains("abc123"), "stored row leaks secret: {raw}");
        assert!(raw.contains("secret_weather"), "secret_ref preserved");
    }

    #[test]
    fn redaction_drops_request_and_response_bodies() {
        // The network egress vector: method/host/path/status persist, bodies never.
        let redacted = redact_metadata(&json!({
            "method": "POST",
            "host": "api.example.com",
            "path": "/v1/leads",
            "status": 201,
            "request_body": {"name": "Ada", "email": "ada@example.com"},
            "response_body": {"id": "lead-1"}
        }));
        let obj = redacted.as_object().unwrap();
        assert!(!obj.contains_key("request_body"));
        assert!(!obj.contains_key("response_body"));
        assert_eq!(obj.get("request_body_redacted").unwrap(), &Value::Bool(true));
        assert_eq!(obj.get("response_body_redacted").unwrap(), &Value::Bool(true));
        assert_eq!(obj.get("method").unwrap(), "POST");
        assert_eq!(obj.get("host").unwrap(), "api.example.com");
        // No PII from the bodies survives anywhere in the redacted value.
        let serialized = serde_json::to_string(&redacted).unwrap();
        for leak in ["Ada", "ada@example.com", "lead-1"] {
            assert!(!serialized.contains(leak), "redacted metadata leaks {leak}: {serialized}");
        }
    }

    #[test]
    fn redaction_recurses_into_nested_request_response() {
        // A `body` nested under `request`/`response` is still dropped.
        let redacted = redact_metadata(&json!({
            "request": {"method": "POST", "body": {"secret": "leak"}},
            "response": {"status": 200, "body": {"token": "leak2"}}
        }));
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains("leak"), "nested body leaked: {serialized}");
        assert!(serialized.contains("POST"));
        assert!(serialized.contains("200"));
    }

    #[test]
    fn highest_audit_seq_tracks_appends() {
        let mut s = store();
        assert_eq!(s.highest_audit_seq().unwrap(), 0);
        s.append_audit(&deny_record(1, "a")).unwrap();
        assert_eq!(s.highest_audit_seq().unwrap(), 1);
        s.append_audit(&deny_record(2, "a")).unwrap();
        assert_eq!(s.highest_audit_seq().unwrap(), 2);
    }
}
