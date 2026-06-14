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
    let allowed: Option<&[Role]> = match cmd.name.as_str() {
        // Owner-only workspace lifecycle (commands.md: workspace.create → Owner).
        "workspace.create" => Some(&[Role::Owner]),
        // Read-level metadata: every member role may open/inspect the workspace.
        "workspace.open" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        // Installing/compiling an applet is a maintainer+ operation (SC-15):
        // Viewer/Auditor/Runner/Editor cannot install.
        "applet.install" => Some(&[Role::Owner, Role::Maintainer]),
        // Triggering execution: the run-capable roles (CR-8). Viewer/Auditor are
        // read-only/oversight and cannot run code.
        "runtime.run" => Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner]),
        // Re-entering an applet's handler on a UI event is *execution* (UI-4/CR-6):
        // same run-capable roles as `runtime.run`. A Viewer/Auditor is read-only and
        // cannot dispatch an event; the capability gate inside the handler then
        // enforces the applet's manifest caps per `ctx.*` call exactly as a run does.
        "ui.dispatch_event" => Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner]),
        // Replay is an audit/oversight operation (CR-9): Auditor/Maintainer/Owner.
        // A bare Runner can run but not replay (per commands.md).
        "runtime.replay" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Session replay is the same audit/oversight operation lifted to an ordered
        // event SESSION (UI-4/CR-6): it replays [initial run + N dispatched events]
        // as one unit, so it carries the same roles as `runtime.replay`.
        "runtime.replay_session" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Reading the records projection requires a read-capable role (db.read).
        "query.execute" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        // Schema evolution (commands.md: schema.apply_change → Owner, Maintainer;
        // DL-8). An additive schema change mutates workspace state, so it is a
        // maintainer+ operation — a Viewer/Editor/Auditor cannot apply one.
        "schema.apply_change" => Some(&[Role::Owner, Role::Maintainer]),
        // Validating compatibility is a read-only check (commands.md:
        // schema.validate_compatibility → Owner, Maintainer, Editor, Auditor).
        "schema.validate_compatibility" => {
            Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Auditor])
        }
        // Rebuilding indexes is a maintenance op (commands.md:
        // schema.rebuild_indexes → Owner, Maintainer; DL-5).
        "schema.rebuild_indexes" => Some(&[Role::Owner, Role::Maintainer]),
        // Exporting the portable workspace bundle (DL-24, commands.md:
        // workspace.export → Owner, Maintainer, Auditor). The Auditor may take a
        // backup/debug bundle (including run logs by policy) without otherwise
        // mutating the workspace.
        "workspace.export" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Importing a bundle reconstructs a workspace (commands.md:
        // workspace.import → Owner). Owner-only because an import writes the whole
        // syncable state (applets, records, grants) into the target.
        "workspace.import" => Some(&[Role::Owner]),
        _ => None,
    };
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
/// shape is a `ValidationError` rather than a silently-ignored grant.
fn payload_db_read_scope(cmd: &CoreCommand) -> Result<Option<Vec<String>>> {
    let grants = match cmd.payload.get("grants") {
        None => return Ok(None),
        Some(g) => g,
    };
    // `grants.db.read` — absent at any level means "no db.read scope supplied".
    let read = grants.get("db").and_then(|db| db.get("read"));
    let read = match read {
        None => return Ok(None),
        Some(r) => r,
    };
    let arr = read.as_array().ok_or_else(|| {
        CoreError::ValidationError(
            "query.execute `grants.db.read` must be an array of collection names".into(),
        )
    })?;
    let mut scopes = Vec::with_capacity(arr.len());
    for entry in arr {
        let s = entry.as_str().ok_or_else(|| {
            CoreError::ValidationError(
                "query.execute `grants.db.read` entries must be collection-name strings".into(),
            )
        })?;
        scopes.push(s.to_string());
    }
    Ok(Some(scopes))
}

/// True iff `collection` is granted by `scope` — either an exact collection-name
/// match or the read-all wildcard `"*"`.
fn scope_grants(scope: &[String], collection: &str) -> bool {
    scope.iter().any(|s| s == "*" || s == collection)
}
