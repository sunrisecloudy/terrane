//! SS-7 remote-op authorization: a **pure** decision over an incoming sync op.
//!
//! This is the M0b apply-time authorization gate (`forge/spec/sync-rbac.md`).
//! Before any opaque CRDT chunk is imported into the receiving store, the
//! receiver must make a deterministic local decision about whether the actor is
//! allowed to author the operation the chunk carries.
//!
//! The trust model is the **same** as the `forge-core` command boundary
//! (`require_db_read`, review 048/050): authorization is derived from the
//! **trusted** receiver-side membership row ([`TrustedMembership`]), never from
//! the incoming message. The optional [`IncomingClaim`] is untrusted session
//! metadata — it is consulted *only* to reject self-escalation (a claim that
//! asserts a role or grant exceeding the trusted membership). A claim can never
//! widen authorization; the trusted row always decides access.
//!
//! [`authorize_remote_op`] is pure (no I/O, no store access) so it can be unit-
//! and fixture-tested in isolation. Phase 2 wires it ahead of the CRDT import in
//! the sync apply path; this module deliberately does **not** touch that path.

use forge_domain::Role;

/// The receiving workspace's **trusted** membership row for the authenticated
/// session actor. This is the SOURCE OF TRUTH for authorization — it is resolved
/// from the local membership table, never from the incoming message
/// (`forge/spec/sync-rbac.md` "Trust boundary").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedMembership {
    /// Authenticated peer actor for the session.
    pub actor_id: String,
    /// Trusted role from the receiving workspace membership table.
    pub role: Role,
    /// Trusted collection read grants (`"*"` = all collections).
    pub db_read: Vec<String>,
    /// Trusted collection write grants (`"*"` = all collections).
    pub db_write: Vec<String>,
    /// Trusted schema-maintenance grant.
    pub schema_write: bool,
}

/// Resource the incoming op targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Record,
    Schema,
}

/// The operation the incoming chunk carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOp {
    Insert,
    Patch,
    Delete,
    SchemaChange,
    Read,
}

impl RemoteOp {
    /// True for ops that author a remote **record write** (insert/patch/delete).
    fn is_record_write(self) -> bool {
        matches!(self, RemoteOp::Insert | RemoteOp::Patch | RemoteOp::Delete)
    }

    /// Audit-action suffix for this op, e.g. `insert` → `sync.record.insert`.
    fn action(self, resource: ResourceType) -> &'static str {
        match (resource, self) {
            (ResourceType::Record, RemoteOp::Insert) => "sync.record.insert",
            (ResourceType::Record, RemoteOp::Patch) => "sync.record.patch",
            (ResourceType::Record, RemoteOp::Delete) => "sync.record.delete",
            (ResourceType::Record, RemoteOp::Read) => "sync.record.read",
            (ResourceType::Schema, RemoteOp::SchemaChange) => "sync.schema.change",
            (ResourceType::Schema, RemoteOp::Read) => "sync.schema.read",
            // Mismatched (record schema_change / schema insert…) — name by op so
            // the audit still records what was attempted.
            (_, RemoteOp::SchemaChange) => "sync.schema.change",
            (_, RemoteOp::Insert) => "sync.record.insert",
            (_, RemoteOp::Patch) => "sync.record.patch",
            (_, RemoteOp::Delete) => "sync.record.delete",
        }
    }
}

/// The semantic envelope that must be inspected before chunk import:
/// `incoming.metadata` + `incoming.record_op` (`forge/spec/sync-rbac.md`
/// "Fixture semantics"). Carries no opaque CRDT bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOpEnvelope {
    pub resource_type: ResourceType,
    pub op: RemoteOp,
    /// Target collection for record ops. `None` for schema ops.
    pub collection: Option<String>,
    /// Target record id for record ops. `None` for schema ops.
    pub record_id: Option<String>,
    /// Target schema id for schema ops. `None` for record ops.
    pub schema_id: Option<String>,
    /// Schema version named in the envelope metadata, if present.
    pub schema_version: Option<u64>,
}

/// An **untrusted** role/grant claim carried in the incoming message/session
/// metadata. It is consulted ONLY to detect self-escalation (a claim that
/// exceeds the trusted membership). It never authorizes anything on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingClaim {
    pub actor_id: String,
    pub role: Role,
    pub db_read: Vec<String>,
    pub db_write: Vec<String>,
    pub schema_write: bool,
}

/// An auditable record of the authorization decision for one remote op. Mirrors
/// the spec's denial requirements (`forge/spec/sync-rbac.md`): actor id,
/// operation, resource, collection or schema id, trusted role, trusted grants,
/// and reason. Written for both allow and deny so the apply boundary is fully
/// auditable (SC-12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAuditRecord {
    /// Audit action, e.g. `sync.record.insert` / `sync.schema.change`.
    pub action: String,
    /// `"allow"` or `"deny"`.
    pub decision: &'static str,
    /// Authenticated actor id (from the trusted membership row).
    pub actor_id: String,
    /// Resource type the op targeted.
    pub resource_type: ResourceType,
    /// Target collection (record ops) or schema id (schema ops).
    pub resource_id: Option<String>,
    /// Collection named in the envelope, if any.
    pub collection: Option<String>,
    /// Schema id named in the envelope, if any.
    pub schema_id: Option<String>,
    /// Trusted role used for the decision.
    pub trusted_role: Role,
    /// Trusted read grants used for the decision.
    pub trusted_db_read: Vec<String>,
    /// Trusted write grants used for the decision.
    pub trusted_db_write: Vec<String>,
    /// Trusted schema-maintenance grant used for the decision.
    pub trusted_schema_write: bool,
    /// Human-readable reason naming the decisive check.
    pub reason: String,
}

/// The outcome of [`authorize_remote_op`]: allow or deny, each carrying the
/// reason and the [`SyncAuditRecord`] to persist. A deny means the apply path
/// must skip the CRDT import, leave projections unchanged, and surface a
/// sync-level `permission_denied`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAuthDecision {
    Allow { reason: String, audit: SyncAuditRecord },
    Deny { reason: String, audit: SyncAuditRecord },
}

impl SyncAuthDecision {
    /// True if this is an allow decision.
    pub fn is_allow(&self) -> bool {
        matches!(self, SyncAuthDecision::Allow { .. })
    }

    /// The audit record carried by the decision (allow or deny).
    pub fn audit(&self) -> &SyncAuditRecord {
        match self {
            SyncAuthDecision::Allow { audit, .. } | SyncAuthDecision::Deny { audit, .. } => audit,
        }
    }

    /// The decision reason (allow or deny).
    pub fn reason(&self) -> &str {
        match self {
            SyncAuthDecision::Allow { reason, .. } | SyncAuthDecision::Deny { reason, .. } => reason,
        }
    }
}

/// Authorize one incoming remote op against the **trusted** membership row,
/// implementing the `forge/spec/sync-rbac.md` apply-time decision order:
///
/// 1. **Self-escalation** — if the (untrusted) `claim` asserts a role or grant
///    exceeding `trusted`, reject before any operation-specific check.
/// 2. **Role matrix** — record insert/patch/delete need Owner/Maintainer/Editor;
///    schema change needs Owner/Maintainer; read-only catch-up allows all roles.
/// 3. **Trusted collection grant** — writes need `db.write` for the collection
///    (`"*"` = all, else exact name); reads need `db.read`.
/// 4. **Schema grant** — schema changes need `schema_write = true`.
///
/// Authorization is derived ENTIRELY from `trusted`. `claim` is consulted only
/// in step 1; it can never widen access. Runner/Viewer/Auditor/Reviewer cannot
/// author remote writes even if the claim asserts write grants.
pub fn authorize_remote_op(
    trusted: &TrustedMembership,
    claim: Option<&IncomingClaim>,
    env: &RemoteOpEnvelope,
) -> SyncAuthDecision {
    let action = env.op.action(env.resource_type).to_string();
    let resource_id = match env.resource_type {
        ResourceType::Record => env.collection.clone(),
        ResourceType::Schema => env.schema_id.clone(),
    };
    let mk_audit = |decision: &'static str, reason: &str| SyncAuditRecord {
        action: action.clone(),
        decision,
        actor_id: trusted.actor_id.clone(),
        resource_type: env.resource_type,
        resource_id: resource_id.clone(),
        collection: env.collection.clone(),
        schema_id: env.schema_id.clone(),
        trusted_role: trusted.role,
        trusted_db_read: trusted.db_read.clone(),
        trusted_db_write: trusted.db_write.clone(),
        trusted_schema_write: trusted.schema_write,
        reason: reason.to_string(),
    };
    let allow = |reason: String| SyncAuthDecision::Allow {
        audit: mk_audit("allow", &reason),
        reason,
    };
    let deny = |reason: String| SyncAuthDecision::Deny {
        audit: mk_audit("deny", &reason),
        reason,
    };

    // (a) Self-escalation: an incoming claim may NARROW trusted grants but never
    // widen them. Any claim role/grant beyond the trusted row is rejected before
    // any operation-specific allow check (spec step 2).
    if let Some(claim) = claim {
        if role_rank(claim.role) > role_rank(trusted.role) {
            return deny(format!(
                "incoming role claim exceeds trusted {} membership",
                role_name(trusted.role)
            ));
        }
        if let Some(extra) = scope_exceeds(&claim.db_write, &trusted.db_write) {
            return deny(format!(
                "incoming db.write claim exceeds trusted membership ({extra})"
            ));
        }
        if let Some(extra) = scope_exceeds(&claim.db_read, &trusted.db_read) {
            return deny(format!(
                "incoming db.read claim exceeds trusted membership ({extra})"
            ));
        }
        // NOTE: a `schema_write` claim that exceeds the trusted row is NOT a
        // standalone rejection. The trusted `schema_write = false` denial
        // surfaces naturally through the schema role/grant check below
        // (`forge/spec/sync-rbac.md`: "incoming schema_write claim is ignored
        // because trusted_peer.schema_write is false"). Rejecting it here would
        // mask the role-based reason the contract pins.
    }

    // From here on, only the TRUSTED membership row decides.
    match (env.resource_type, env.op) {
        // ---- Record writes (insert / patch / delete) -----------------------
        (ResourceType::Record, op) if op.is_record_write() => {
            // (b) Role matrix: only Owner/Maintainer/Editor may author writes.
            if !role_can_record_write(trusted.role) {
                let reason = match trusted.role {
                    Role::Runner => {
                        "runner role does not imply remote record write".to_string()
                    }
                    other => format!(
                        "{} cannot author remote record writes",
                        role_name(other)
                    ),
                };
                return deny(reason);
            }
            // (c) Trusted db.write grant for the collection.
            let collection = env.collection.as_deref().unwrap_or("");
            if scope_is_wildcard(&trusted.db_write) {
                return allow(format!(
                    "{} wildcard db.write covers {collection}",
                    role_name(trusted.role)
                ));
            }
            if scope_grants(&trusted.db_write, collection) {
                return allow(format!(
                    "trusted {} has db.write on {collection}",
                    role_name(trusted.role)
                ));
            }
            deny(format!("trusted db.write does not include {collection}"))
        }

        // ---- Schema change -------------------------------------------------
        (ResourceType::Schema, RemoteOp::SchemaChange) => {
            // (b) Role matrix: only Owner/Maintainer may change schema.
            if !role_can_schema_change(trusted.role) {
                return deny(format!(
                    "{} lacks trusted schema_write",
                    role_name(trusted.role)
                ));
            }
            // (d) Trusted schema_write grant.
            if !trusted.schema_write {
                return deny(format!(
                    "{} lacks trusted schema_write",
                    role_name(trusted.role)
                ));
            }
            allow(format!("{} has trusted schema_write", role_name(trusted.role)))
        }

        // ---- Read-only catch-up (any role) ---------------------------------
        (_, RemoteOp::Read) => {
            let collection = env.collection.as_deref().unwrap_or("");
            if scope_is_wildcard(&trusted.db_read) {
                return allow(format!(
                    "{} wildcard db.read covers {collection}",
                    role_name(trusted.role)
                ));
            }
            if scope_grants(&trusted.db_read, collection) {
                return allow(format!(
                    "trusted {} has db.read on {collection}",
                    role_name(trusted.role)
                ));
            }
            deny(format!("trusted db.read does not include {collection}"))
        }

        // ---- Mismatched resource/op (e.g. record schema_change) ------------
        (resource, op) => deny(format!(
            "unsupported remote op {op:?} for resource {resource:?}"
        )),
    }
}

/// Ordinal rank for self-escalation comparison: a claim whose role ranks higher
/// than the trusted role is a widening attempt. Owner is the most privileged.
fn role_rank(role: Role) -> u8 {
    match role {
        Role::Owner => 6,
        Role::Maintainer => 5,
        Role::Editor => 4,
        Role::Reviewer => 3,
        Role::Auditor => 2,
        Role::Runner => 1,
        Role::Viewer => 0,
    }
}

/// snake_case role name for audit reasons (matches the fixture wording).
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Maintainer => "maintainer",
        Role::Editor => "editor",
        Role::Runner => "runner",
        Role::Viewer => "viewer",
        Role::Auditor => "auditor",
        Role::Reviewer => "reviewer",
    }
}

/// Roles that may author a remote record write (insert/patch/delete).
fn role_can_record_write(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Maintainer | Role::Editor)
}

/// Roles that may author a remote schema change.
fn role_can_schema_change(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Maintainer)
}

/// True iff `scope` is the read-all/write-all wildcard.
fn scope_is_wildcard(scope: &[String]) -> bool {
    scope.iter().any(|s| s == "*")
}

/// True iff `collection` is granted by `scope` — exact name or `"*"` wildcard.
fn scope_grants(scope: &[String], collection: &str) -> bool {
    scope.iter().any(|s| s == "*" || s == collection)
}

/// If `claim` grants any collection not covered by `trusted`, return that
/// offending entry (a self-escalation). `None` means `claim ⊆ trusted`. A
/// wildcard trusted scope covers everything.
fn scope_exceeds(claim: &[String], trusted: &[String]) -> Option<String> {
    if scope_is_wildcard(trusted) {
        return None;
    }
    claim
        .iter()
        .find(|entry| !scope_grants(trusted, entry))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_env(op: RemoteOp, collection: &str) -> RemoteOpEnvelope {
        RemoteOpEnvelope {
            resource_type: ResourceType::Record,
            op,
            collection: Some(collection.to_string()),
            record_id: Some("rec-1".to_string()),
            schema_id: None,
            schema_version: Some(1),
        }
    }

    fn member(role: Role, write: &[&str], schema_write: bool) -> TrustedMembership {
        TrustedMembership {
            actor_id: "actor-1".to_string(),
            role,
            db_read: write.iter().map(|s| s.to_string()).collect(),
            db_write: write.iter().map(|s| s.to_string()).collect(),
            schema_write,
        }
    }

    #[test]
    fn editor_with_grant_allows_insert() {
        let trusted = member(Role::Editor, &["tasks"], false);
        let env = record_env(RemoteOp::Insert, "tasks");
        let d = authorize_remote_op(&trusted, None, &env);
        assert!(d.is_allow());
        assert_eq!(d.audit().action, "sync.record.insert");
        assert!(d.reason().contains("db.write on tasks"));
    }

    #[test]
    fn editor_outside_scope_denies() {
        let trusted = member(Role::Editor, &["notes"], false);
        let env = record_env(RemoteOp::Patch, "tasks");
        let d = authorize_remote_op(&trusted, None, &env);
        assert!(!d.is_allow());
        assert!(d.reason().contains("does not include tasks"));
    }

    #[test]
    fn viewer_write_denied_by_role() {
        let trusted = member(Role::Viewer, &["tasks"], false);
        let env = record_env(RemoteOp::Insert, "tasks");
        let d = authorize_remote_op(&trusted, None, &env);
        assert!(!d.is_allow());
    }

    #[test]
    fn self_escalation_rejected_before_op_check() {
        let trusted = member(Role::Viewer, &[], false);
        let claim = IncomingClaim {
            actor_id: "actor-1".to_string(),
            role: Role::Owner,
            db_read: vec!["*".to_string()],
            db_write: vec!["*".to_string()],
            schema_write: true,
        };
        let env = record_env(RemoteOp::Insert, "tasks");
        let d = authorize_remote_op(&trusted, Some(&claim), &env);
        assert!(!d.is_allow());
        assert!(d.reason().contains("exceeds trusted viewer membership"));
    }
}
