//! CR-A3 command-level RBAC authorization for [`WorkspaceCore::handle`].
//!
//! prd-merged/01 CR-A3 ("every command carries an [`ActorContext`] and passes
//! policy before touching state"). [`authorize`] is the first of the two CR-A3
//! layers: a command-level role gate that rejects an actor whose role is not
//! permitted to issue the command per `forge/spec/commands.md`, BEFORE dispatch
//! touches any state. The second layer — the per-`ctx.*` capability/policy gate —
//! still runs at host-call time inside the runtime.
//!
//! The `query.execute` path adds a collection-scoped `db.read` capability gate
//! ([`require_db_read`]): an ordered pipeline of role check → payload self-
//! escalation rejection ([`reject_payload_self_escalation`] BEFORE
//! [`scope_grants`]) → trusted-scope grant check. The trusted scope is the
//! authorization source; a request payload can only ever narrow it and a payload
//! that tries to exceed it is rejected as a self-escalation (review 048).

use crate::catalog::descriptor_for;
use forge_domain::{CoreCommand, CoreError, Result, Role};

/// Command-level RBAC gate (prd-merged/01 CR-A3): reject a command whose
/// actor role is not permitted to issue it *before* any handler touches state.
///
/// The role matrix is the "Roles" column of `forge/spec/commands.md` for the
/// M0a command set. An unauthorized actor returns [`CoreError::PermissionDenied`];
/// an unknown command is left for the dispatcher to reject with a
/// `ValidationError` (so capability negotiation, not authorization, owns that
/// message). This is the first of the two CR-A3 layers; the per-`ctx.*`
/// capability/policy gate still runs at host-call time inside the runtime.
pub(super) fn authorize(cmd: &CoreCommand) -> Result<()> {
    let role = cmd.actor.role;
    // `None` ⇒ no command-level role restriction (the command is reachable by
    // any authenticated actor, or it is an unknown name the dispatcher rejects).
    // Role sets are the single source of truth in the command catalog (cli-plan P1.4).
    let allowed = descriptor_for(cmd.name.as_str()).map(|entry| entry.required_roles);
    match allowed {
        Some(roles) if !roles.contains(&role) => Err(CoreError::PermissionDenied(format!(
            "actor role {role:?} is not permitted to issue {:?} (see forge/spec/commands.md)",
            cmd.name
        ))),
        _ => Ok(()),
    }
}

/// True iff `role` carries the `db.read` capability at the command level.
///
/// `forge/spec/commands.md` lists the data-read membership roles (the same set
/// that may `workspace.open` / `file.history` / read projections): Owner,
/// Maintainer, Editor, Viewer, Auditor. The execution-only `Runner` and the
/// code-review `Reviewer` are NOT data readers, so they lack `db.read` even
/// though `Runner` may `runtime.run`. This mirrors the manifest `db.read` grant
/// the runtime enforces per `ctx.db.*` call, lifted to the workspace command.
fn role_has_db_read(role: Role) -> bool {
    matches!(
        role,
        Role::Owner | Role::Maintainer | Role::Editor | Role::Viewer | Role::Auditor
    )
}

/// Collection-scoped `db.read` capability gate for `query.execute` (review
/// 036/038/048 finding 1; `forge/spec/commands.md:21` "Role plus db.read
/// capability" + `forge/spec/capabilities.md:23` `resource: collection:<name>`).
///
/// Two independent checks, both required:
///
///   1. **Role** — the actor's role must carry `db.read` ([`role_has_db_read`]).
///      A `Runner` (execution-only) fails here with `PermissionDenied`.
///   2. **Scope** — the target `collection` must be within the caller's granted
///      `db.read` scope. `trusted_scope` is the workspace-side grant for this
///      actor (`Some(&["tasks"])`, `Some(&["*"])` for read-all, or `Some(&[])` for
///      "no collection granted"), resolved by the caller from the TRUSTED grant
///      table — **never** from the request payload (review 048 finding 1). A
///      collection outside the granted scope is `CapabilityRequired` with a
///      message naming `db.read collection:<name>`, so a role that cleared check 1
///      is still denied when it was not granted that specific collection (this is
///      what makes the capability layer load-bearing rather than redundant with
///      the role gate, AND unforgeable from the command body).
///
/// Back-compat: when the actor has **no** trusted grant entry (`None`), the scope
/// defaults to the role-derived read scope (read-all for a `db.read`-capable
/// role), so the existing owner-permits-all spine — which provisions no grants —
/// keeps working. To exercise a narrowed scope, configure it through
/// [`WorkspaceCore::grant_db_read`].
///
/// Defense in depth: a request payload that smuggles its own `grants.db.read`
/// scope is treated as untrusted input. It can only ever *narrow* (it cannot
/// widen the trusted grant), and a payload grant that tries to exceed the trusted
/// scope is rejected as a `PermissionDenied` self-escalation attempt rather than
/// silently honored.
pub(super) fn require_db_read(cmd: &CoreCommand, collection: &str, trusted_scope: Option<&[String]>) -> Result<()> {
    // Layer 1: role.
    if !role_has_db_read(cmd.actor.role) {
        return Err(CoreError::PermissionDenied(format!(
            "actor role {:?} lacks the db.read capability required to query {collection:?} (forge/spec/commands.md: query.execute = Role plus db.read)",
            cmd.actor.role
        )));
    }

    // A payload-supplied `grants.db.read` is untrusted: validate its shape and
    // ensure it does not attempt to exceed the trusted grant. It is NEVER the
    // authorization source.
    reject_payload_self_escalation(cmd, trusted_scope)?;

    // Layer 2: collection-scoped grant, evaluated against the TRUSTED scope only.
    match trusted_scope {
        // No trusted grant entry → role-derived read-all (back-compat).
        None => Ok(()),
        Some(scope) if scope_grants(scope, collection) => Ok(()),
        Some(_) => Err(CoreError::CapabilityRequired(format!(
            "db.read collection:{collection} is not within the caller's granted db.read scope (forge/spec/capabilities.md: db.read is collection-scoped)"
        ))),
    }
}

/// Reject a request whose payload `grants.db.read` tries to grant the caller MORE
/// than its trusted scope (a self-escalation). The payload grant is never used to
/// authorize; this only refuses an attempt to widen access through the command
/// body, and validates the grant shape. A payload that is absent, malformed, or a
/// subset of the trusted scope passes (the trusted scope still decides access).
fn reject_payload_self_escalation(cmd: &CoreCommand, trusted_scope: Option<&[String]>) -> Result<()> {
    let payload_scope = match payload_db_read_scope(cmd)? {
        None => return Ok(()),
        Some(scope) => scope,
    };
    // With no trusted entry the actor is role-derived read-all, so any payload
    // scope is a (redundant) narrowing — nothing to escalate past.
    let Some(trusted) = trusted_scope else {
        return Ok(());
    };
    // Read-all trusted scope can never be exceeded.
    if trusted.iter().any(|s| s == "*") {
        return Ok(());
    }
    // Any payload entry not covered by the trusted scope is an escalation attempt.
    for entry in &payload_scope {
        if !scope_grants(trusted, entry) {
            return Err(CoreError::PermissionDenied(format!(
                "query.execute payload requested db.read collection:{entry} beyond the actor's trusted grant; the db.read scope is set by the workspace, not the request (review 048)"
            )));
        }
    }
    Ok(())
}

/// Parse a payload-supplied `db.read` scope from `payload.grants.db.read`, if
/// present. `Ok(None)` means no scope was supplied; `Ok(Some(scopes))` is the
/// (untrusted) list of collection names (`"*"` = read-all). A malformed `grants`
/// shape is a `ValidationError` rather than a silently-ignored grant. Thin wrapper
/// over [`payload_db_scope`] (shared with the `db.write` self-escalation check).
fn payload_db_read_scope(cmd: &CoreCommand) -> Result<Option<Vec<String>>> {
    payload_db_scope(cmd, "read")
}

/// True iff `role` carries the `db.write` capability at the command level.
///
/// `forge/spec/commands.md` lists the data-WRITE membership roles — the `record.put`
/// / `record.patch` / `record.delete` writers and the `file.restore_version` analog
/// of `db.restore`: Owner, Maintainer, Editor. A read-only `Viewer` / oversight
/// `Auditor`, the execution-only `Runner`, and the code-review `Reviewer` are NOT
/// data writers, so they lack `db.write` even though a `Viewer`/`Auditor` may read.
/// This mirrors the manifest `db.write` grant the runtime enforces per `ctx.db.*`
/// call, lifted to the workspace command.
fn role_has_db_write(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Maintainer | Role::Editor)
}

/// Collection-scoped `db.write` capability gate for `db.restore` (DL-20; the write
/// counterpart of [`require_db_read`]). A non-destructive restore appends a new
/// record version, i.e. it is a record WRITE, so `forge/spec/commands.md`'s
/// "Role plus db.write capability" + `forge/spec/capabilities.md:23` `db.write`
/// (`resource: collection:<name>`) gate it identically to a record write.
///
/// Two independent checks, both required:
///
///   1. **Role** — the actor's role must carry `db.write` ([`role_has_db_write`]).
///      A `Viewer`/`Auditor`/`Runner` fails here with `PermissionDenied`.
///   2. **Scope** — the target `collection` must be within the caller's granted
///      `db.write` scope. `trusted_scope` is the workspace-side grant for this actor
///      (`Some(&["tasks"])`, `Some(&["*"])` for write-all, or `Some(&[])` for "no
///      collection granted"), resolved by the caller from the TRUSTED grant table —
///      **never** from the request payload (review 048/050). A collection outside the
///      granted scope is `CapabilityRequired` naming `db.write collection:<name>`.
///
/// Back-compat: an actor with **no** trusted grant entry (`None`) defaults to the
/// role-derived write scope (write-all for a `db.write`-capable role), so the
/// owner-permits-all spine — which provisions no grants — keeps working. Configure a
/// narrowed scope through [`WorkspaceCore::grant_db_write`].
///
/// Defense in depth: a payload-supplied `grants.db.write` scope is untrusted input.
/// It can only ever *narrow* the trusted grant, and a payload grant that tries to
/// exceed the trusted scope is rejected as a `PermissionDenied` self-escalation.
pub(super) fn require_db_write(
    cmd: &CoreCommand,
    collection: &str,
    trusted_scope: Option<&[String]>,
) -> Result<()> {
    // Layer 1: role.
    if !role_has_db_write(cmd.actor.role) {
        return Err(CoreError::PermissionDenied(format!(
            "actor role {:?} lacks the db.write capability required to restore in {collection:?} (forge/spec/commands.md: db.restore = Role plus db.write)",
            cmd.actor.role
        )));
    }

    // A payload-supplied `grants.db.write` is untrusted: validate its shape and
    // ensure it does not attempt to exceed the trusted grant. It is NEVER the
    // authorization source.
    reject_payload_write_self_escalation(cmd, trusted_scope)?;

    // Layer 2: collection-scoped grant, evaluated against the TRUSTED scope only.
    match trusted_scope {
        None => Ok(()),
        Some(scope) if scope_grants(scope, collection) => Ok(()),
        Some(_) => Err(CoreError::CapabilityRequired(format!(
            "db.write collection:{collection} is not within the caller's granted db.write scope (forge/spec/capabilities.md: db.write is collection-scoped)"
        ))),
    }
}

/// Reject a request whose payload `grants.db.write` tries to grant the caller MORE
/// than its trusted scope (a self-escalation). The write counterpart of
/// [`reject_payload_self_escalation`]; the payload grant is never used to authorize.
fn reject_payload_write_self_escalation(
    cmd: &CoreCommand,
    trusted_scope: Option<&[String]>,
) -> Result<()> {
    let payload_scope = match payload_db_scope(cmd, "write")? {
        None => return Ok(()),
        Some(scope) => scope,
    };
    let Some(trusted) = trusted_scope else {
        return Ok(());
    };
    if trusted.iter().any(|s| s == "*") {
        return Ok(());
    }
    for entry in &payload_scope {
        if !scope_grants(trusted, entry) {
            return Err(CoreError::PermissionDenied(format!(
                "db.restore payload requested db.write collection:{entry} beyond the actor's trusted grant; the db.write scope is set by the workspace, not the request (review 048)"
            )));
        }
    }
    Ok(())
}

/// Parse a payload-supplied `db.<action>` scope from `payload.grants.db.<action>`,
/// if present. The shared parser behind [`payload_db_read_scope`] and the
/// `db.write` self-escalation check. `Ok(None)` means no scope was supplied;
/// `Ok(Some(scopes))` is the (untrusted) list of collection names (`"*"` =
/// all). A malformed `grants` shape is a `ValidationError`.
fn payload_db_scope(cmd: &CoreCommand, action: &str) -> Result<Option<Vec<String>>> {
    let grants = match cmd.payload.get("grants") {
        None => return Ok(None),
        Some(g) => g,
    };
    let scoped = grants.get("db").and_then(|db| db.get(action));
    let scoped = match scoped {
        None => return Ok(None),
        Some(r) => r,
    };
    let arr = scoped.as_array().ok_or_else(|| {
        CoreError::ValidationError(format!(
            "`grants.db.{action}` must be an array of collection names"
        ))
    })?;
    let mut scopes = Vec::with_capacity(arr.len());
    for entry in arr {
        let s = entry.as_str().ok_or_else(|| {
            CoreError::ValidationError(format!(
                "`grants.db.{action}` entries must be collection-name strings"
            ))
        })?;
        scopes.push(s.to_string());
    }
    Ok(Some(scopes))
}

/// True iff `collection` is granted by `scope` — either an exact collection-name
/// match or the all wildcard `"*"`.
fn scope_grants(scope: &[String], collection: &str) -> bool {
    scope.iter().any(|s| s == "*" || s == collection)
}
