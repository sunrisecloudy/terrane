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

pub(crate) mod registry;

pub(super) mod applet;
pub(super) mod audit;
pub(super) mod bridge;
#[cfg(feature = "control")]
pub(super) mod control;
pub(super) mod legacy_core_step;
pub(super) mod package;
pub(super) mod lifecycle;
pub(super) mod query;
pub(super) mod quota;
pub(super) mod replay;
pub(super) mod runtime_run;
pub(super) mod schema;
pub(super) mod system;
pub(super) mod sync;
pub(super) mod test_hooks;
pub(super) mod time_travel;
pub(super) mod ui;
pub(super) mod watch;
pub(super) mod workspace_export;

use self::registry::{CommandRegistration, OUTER_COMMANDS};

/// The command registry: maps a folded [`CommandRegistration`] row to dispatch.
/// Built once and consulted by [`WorkspaceCore::handle`] AFTER the CR-A3
/// authorization gate — the registry owns ONLY routing, never authorization or
/// lifecycle gating (those stay in the handlers / the facade exactly as before).
pub(super) struct Registry {
    table: &'static [CommandRegistration],
}

impl Registry {
    /// The process-wide command registry over the static folded catalog.
    pub(super) fn new() -> Self {
        Registry {
            table: OUTER_COMMANDS,
        }
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
        if let Some(entry) = self
            .table
            .iter()
            .find(|entry| entry.descriptor.name == name)
        {
            return (entry.handler)(core, cmd);
        }
        #[cfg(feature = "control")]
        if let Some(entry) = self::registry::CONTROL_COMMANDS
            .iter()
            .find(|entry| entry.descriptor.name == name)
        {
            return (entry.handler)(core, cmd);
        }
        Err(CoreError::ValidationError(format!(
            "unknown command {name:?} (CR-A5: client should negotiate capability)"
        )))
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

#[cfg(test)]
mod registry_catalog_sync {
    use crate::catalog::{catalog_entries, descriptor_for};
    use super::registry::{CommandRegistration, OUTER_COMMANDS};
    #[cfg(feature = "control")]
    use super::registry::CONTROL_COMMANDS;
    use forge_domain::{catalog::CommandVisibility, Role};
    use std::collections::BTreeSet;
    use std::path::Path;

    fn all_registrations() -> Vec<&'static CommandRegistration> {
        let mut rows: Vec<&'static CommandRegistration> = OUTER_COMMANDS.iter().collect();
        #[cfg(feature = "control")]
        rows.extend(CONTROL_COMMANDS.iter());
        rows
    }

    #[test]
    fn folded_registry_names_match_descriptors() {
        for entry in all_registrations() {
            let found = descriptor_for(entry.descriptor.name)
                .expect("descriptor lookup must succeed for folded row");
            assert_eq!(found.name, entry.descriptor.name);
        }
    }

    #[test]
    fn every_catalog_entry_has_a_folded_handler() {
        let registered: BTreeSet<&str> = all_registrations()
            .iter()
            .map(|entry| entry.descriptor.name)
            .collect();
        for entry in catalog_entries() {
            assert!(
                registered.contains(entry.name),
                "orphan catalog entry {name} has no handler",
                name = entry.name
            );
        }
    }

    #[test]
    fn referenced_schema_files_exist() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for entry in catalog_entries() {
            for path in [entry.payload_schema, entry.response_schema]
                .into_iter()
                .flatten()
            {
                let full = repo_root.join(path);
                assert!(full.is_file(), "missing schema file {}", full.display());
            }
        }
    }

    #[test]
    fn public_commands_are_broadly_reachable() {
        let privileged_only = [Role::Owner];
        for entry in catalog_entries() {
            if entry.visibility != CommandVisibility::Public {
                continue;
            }
            let only_owner = entry.required_roles.len() == 1
                && entry.required_roles == privileged_only;
            assert!(
                !only_owner,
                "public command {} requires Owner only",
                entry.name
            );
        }
    }
}
