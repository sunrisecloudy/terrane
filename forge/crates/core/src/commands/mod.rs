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
//!   - [`workspace_export`] — `workspace.export` / `workspace.import` (DL-24).
//!
//! Every handler is an `impl WorkspaceCore` method (or a free fn over its state),
//! moved here VERBATIM from `workspace.rs` (a pure move, /simplify #11a): the
//! `handle` match still calls `self.cmd_*()` exactly as before, so dispatch
//! semantics — RBAC before dispatch, the unknown-command reject (CR-A5), the
//! lifecycle suspension gate — are byte-for-byte unchanged; only the bodies moved.

use forge_domain::{AppletId, CoreCommand, CoreError, Result};

pub(super) mod applet;
pub(super) mod query;
pub(super) mod replay;
pub(super) mod runtime_run;
pub(super) mod schema;
pub(super) mod ui;
pub(super) mod workspace_export;

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
