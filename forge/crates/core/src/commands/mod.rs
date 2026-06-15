//! The per-command handlers behind [`WorkspaceCore::handle`](super::WorkspaceCore)
//! (prd-merged/04 P-04 command catalog, `forge/spec/commands.md`).
//!
//! Each command in the M0a catalog gets its own focused module so the facade
//! (`workspace.rs`) reads as orchestration — the [`handle`](super::WorkspaceCore)
//! dispatch table plus the shared `WorkspaceCore` state — while each handler's
//! body lives next to the feature it implements:
//!
//!   - [`applet`] — `applet.install` (compile + sign-verify + store);
//!   - [`runtime_run`] — `runtime.run` (record a deterministic run);
//!   - [`replay`] — `runtime.replay` + the version-pinned replay machinery;
//!   - [`ui`] — `ui.dispatch_event` + `runtime.replay_session` (the interactive
//!     loop and its session-replay analogue);
//!   - [`schema`] — `schema.apply_change` / `validate_compatibility` /
//!     `rebuild_indexes` (DL-7/DL-8 → DL-5);
//!   - [`query`] — `query.execute`;
//!   - [`audit`] — `audit.query` (the privileged READ over the SC-12 audit log);
//!   - [`workspace_export`] — `workspace.export` / `workspace.import` (DL-24).
//!
//! Every handler is an `impl WorkspaceCore` method (or a free fn over its state),
//! moved here VERBATIM from `workspace.rs` (a pure move, /simplify #11a): the
//! `handle` match still calls `self.cmd_*()` exactly as before, so dispatch
//! semantics — RBAC before dispatch, the unknown-command reject (CR-A5), the
//! lifecycle suspension gate — are byte-for-byte unchanged; only the bodies moved.

use forge_domain::{AppletId, CoreCommand, CoreError, Result};

use super::WorkspaceCore;

pub(super) mod applet;
pub(super) mod audit;
pub(super) mod lifecycle;
pub(super) mod query;
pub(super) mod quota;
pub(super) mod replay;
pub(super) mod runtime_run;
pub(super) mod schema;
pub(super) mod sync;
pub(super) mod test_hooks;
pub(super) mod time_travel;
pub(super) mod ui;
pub(super) mod watch;
pub(super) mod workspace_export;

/// One command handler: a method over [`WorkspaceCore`] state, taken as a function
/// pointer so the registry can hold the whole catalog in one table. Every M0a
/// handler shares this signature (`&mut self, &CoreCommand -> Result<Value>`), so a
/// `cmd_*` method coerces directly to this type — the registry is just the old
/// `handle` match arms turned into data (`/simplify #11b`).
type Handler = fn(&mut WorkspaceCore, &CoreCommand) -> Result<serde_json::Value>;

/// The command catalog as DATA: command name → handler, built ONCE as a static
/// table (prd-merged/04 P-04, `forge/spec/commands.md`). Each entry is exactly one
/// old `handle` match arm — `"name" => self.cmd_x(&cmd)` becomes
/// `("name", WorkspaceCore::cmd_x)` — so [`Registry::dispatch`] produces the SAME
/// routing as the hand-written match it replaces, and an unregistered name is
/// rejected at the SAME place with the SAME CR-A5 error. Adding a command is now a
/// single row here plus its handler module, with no change to the facade's
/// [`handle`](WorkspaceCore::handle).
///
/// Ordering mirrors the former match for readability only; dispatch is by exact
/// name match, so order is not semantically significant (each name is unique).
const COMMANDS: &[(&str, Handler)] = &[
    ("workspace.create", WorkspaceCore::cmd_workspace_create),
    ("workspace.open", WorkspaceCore::cmd_workspace_open),
    ("applet.install", WorkspaceCore::cmd_applet_install),
    // Applet lifecycle transitions (CR-7, commands/lifecycle.rs): the enable/
    // suspend/uninstall durable-state changes over the installed-applet record +
    // the trusted `AppletLifecycle` flag (`applet.install` mints the enabled v1).
    ("applet.enable", WorkspaceCore::cmd_applet_enable),
    ("applet.suspend", WorkspaceCore::cmd_applet_suspend),
    // `applet.upgrade` (CR-7): atomically install a new version over an active
    // applet (compile + validate + schema additions staged; the active pointer
    // moves to v2 only after all staged work commits; a staged failure rolls back).
    ("applet.upgrade", WorkspaceCore::cmd_applet_upgrade),
    ("applet.uninstall", WorkspaceCore::cmd_applet_uninstall),
    ("runtime.run", WorkspaceCore::cmd_runtime_run),
    ("runtime.replay", WorkspaceCore::cmd_runtime_replay),
    ("runtime.replay_session", WorkspaceCore::cmd_runtime_replay_session),
    ("ui.dispatch_event", WorkspaceCore::cmd_ui_dispatch_event),
    ("query.execute", WorkspaceCore::cmd_query_execute),
    // The privileged READ over the SC-12 durable audit log (commands/audit.rs):
    // return the redacted, append-only rows matching the payload filter, ordered by
    // seq. Gated to the oversight roles in `auth.rs` (reading the security trail is
    // privileged); a role-denied `audit.query` itself lands a command-RBAC audit row.
    ("audit.query", WorkspaceCore::cmd_audit_query),
    // Live queries (DL-16, commands/watch.rs): register/cancel a reactive
    // `db.watch` over a row query. Registration carries the same collection-scoped
    // `db.read` grant as `query.execute`; `db.unwatch` is idempotent.
    ("db.watch", WorkspaceCore::cmd_db_watch),
    ("db.unwatch", WorkspaceCore::cmd_db_unwatch),
    // File-level time travel (DL-20, commands/time_travel.rs): read a record's
    // change feed (`db.history`, gated by collection-scoped `db.read`) and perform a
    // NON-DESTRUCTIVE restore that appends a new version (`db.restore`, gated by
    // collection-scoped `db.write`). Both scope the grant from the trusted context,
    // never the request payload.
    ("db.history", WorkspaceCore::cmd_db_history),
    ("db.restore", WorkspaceCore::cmd_db_restore),
    ("schema.apply_change", WorkspaceCore::cmd_schema_apply_change),
    (
        "schema.validate_compatibility",
        WorkspaceCore::cmd_schema_validate_compatibility,
    ),
    ("schema.rebuild_indexes", WorkspaceCore::cmd_schema_rebuild_indexes),
    // One-ABI CRDT/sync transport (SS-1/SS-2/SS-7): hosts export/import CRDT
    // chunks through `forge_core_handle_command` instead of a second `forge_crdt_*`
    // C surface. `sync.import` authorizes every packet chunk against trusted
    // receiver membership before atomic storage apply.
    ("sync.trust_peer", WorkspaceCore::cmd_sync_trust_peer),
    ("sync.export", WorkspaceCore::cmd_sync_export),
    ("sync.import", WorkspaceCore::cmd_sync_import),
    // Workspace quotas (DL-22, commands/quota.rs): `quota.status` REPORTS usage vs the
    // trusted limits + the approaching-limit warnings (a read, scoped to the whole
    // workspace from trusted state); `quota.set` CONFIGURES the trusted policy override
    // (privileged Owner-only admin — enforcement reads the policy from this persisted
    // state, never the write's payload, so a write cannot widen its own quota).
    ("quota.status", WorkspaceCore::cmd_quota_status),
    ("quota.set", WorkspaceCore::cmd_quota_set),
    ("workspace.export", WorkspaceCore::cmd_workspace_export),
    ("workspace.import", WorkspaceCore::cmd_workspace_import),
];

/// The command registry: maps a command name to its handler over the [`COMMANDS`]
/// table. Built once and consulted by [`WorkspaceCore::handle`] AFTER the CR-A3
/// authorization gate — the registry owns ONLY routing, never authorization or
/// lifecycle gating (those stay in the handlers / the facade exactly as before).
pub(super) struct Registry {
    table: &'static [(&'static str, Handler)],
}

impl Registry {
    /// The process-wide command registry over the static [`COMMANDS`] catalog.
    pub(super) fn new() -> Self {
        Registry { table: COMMANDS }
    }

    /// Route `cmd` to its handler and run it against `core`, returning the handler's
    /// result. This is the dispatch half of the former `handle` match: a registered
    /// name calls the SAME `cmd_*` method it used to; an UNREGISTERED name returns
    /// the IDENTICAL CR-A5 `ValidationError` (same message, same place) — the
    /// graceful-reject contract for an unknown command.
    ///
    /// Authorization is NOT performed here: [`WorkspaceCore::handle`] runs
    /// [`authorize`](super::auth::authorize) BEFORE calling `dispatch`, preserving
    /// the CR-A3 "policy before dispatch" ordering unchanged.
    pub(super) fn dispatch(
        &self,
        core: &mut WorkspaceCore,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let name = cmd.name.as_str();
        match self.table.iter().find(|(n, _)| *n == name) {
            Some((_, handler)) => handler(core, cmd),
            None => Err(CoreError::ValidationError(format!(
                "unknown command {name:?} (CR-A5: client should negotiate capability)"
            ))),
        }
    }
}

/// Extract and require the command's `applet_id` (from the envelope, or the
/// payload as a fallback).
pub(super) fn require_applet_id(cmd: &CoreCommand) -> Result<AppletId> {
    if let Some(id) = &cmd.applet_id {
        return Ok(id.clone());
    }
    cmd.payload
        .get("applet_id")
        .and_then(|v| v.as_str())
        .map(AppletId::new)
        .ok_or_else(|| CoreError::ValidationError(format!("{} requires an applet_id", cmd.name)))
}

/// Deserialize a required object field from the command payload.
pub(super) fn take_field<T: serde::de::DeserializeOwned>(
    cmd: &CoreCommand,
    field: &str,
) -> Result<T> {
    let value = cmd.payload.get(field).ok_or_else(|| {
        CoreError::ValidationError(format!("{} requires a `{field}` field", cmd.name))
    })?;
    serde_json::from_value(value.clone()).map_err(|e| {
        CoreError::ValidationError(format!("{} `{field}` is malformed: {e}", cmd.name))
    })
}

/// Read an optional boolean command field, defaulting to `false` when absent.
/// A present-but-non-boolean value is a `ValidationError` rather than a silent
/// default, so a malformed flag is surfaced.
pub(super) fn bool_field(cmd: &CoreCommand, field: &str) -> Result<bool> {
    match cmd.payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(other) => Err(CoreError::ValidationError(format!(
            "{} `{field}` must be a boolean, got {other}",
            cmd.name
        ))),
    }
}
