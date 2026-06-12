//! forge-domain: shared vocabulary for the forge core.
//!
//! Types here are the hard-to-change contracts every other crate depends on:
//! the error enum, ID newtypes, the command/event/response envelopes
//! (prd-merged/01 CR-A1..A5, adopted from P-04), the record envelope
//! (prd-merged/02 DL §5), the applet manifest + limits (prd-merged/01 CR-5,
//! prd-merged/07 §07-runtime), and the deterministic run record used for
//! replay (prd-merged/01 CR-8/CR-9, CR-11).
//!
//! This crate is pure types + a tiny amount of validation; it has no I/O and
//! must stay `wasm32-unknown-unknown`-clean.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub mod ids;
pub mod manifest;
pub mod record;
pub mod run;

pub use ids::*;
pub use manifest::*;
pub use record::*;
pub use run::*;

/// Stable, typed, user-displayable, machine-actionable error set.
///
/// prd-merged/01-core-runtime-prd.md CR-A4. FFI calls never panic across the
/// boundary; all shell-facing calls return `Result<_, CoreError>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "detail")]
pub enum CoreError {
    #[error("validation error: {0}")]
    ValidationError(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("capability required: {0}")]
    CapabilityRequired(String),
    #[error("storage error: {0}")]
    StorageError(String),
    #[error("schema compatibility error: {0}")]
    SchemaCompatibilityError(String),
    #[error("query error: {0}")]
    QueryError(String),
    #[error("runtime error: {0}")]
    RuntimeError(String),
    #[error("resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),
    #[error("sync error: {0}")]
    SyncError(String),
    #[error("conflict requires user: {0}")]
    ConflictRequiresUser(String),
    #[error("provider error: {0}")]
    ProviderError(String),
    #[error("platform unavailable: {0}")]
    PlatformUnavailable(String),
}

impl CoreError {
    /// Stable machine token for the error kind (for logs/telemetry/tests).
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::ValidationError(_) => "ValidationError",
            CoreError::PermissionDenied(_) => "PermissionDenied",
            CoreError::CapabilityRequired(_) => "CapabilityRequired",
            CoreError::StorageError(_) => "StorageError",
            CoreError::SchemaCompatibilityError(_) => "SchemaCompatibilityError",
            CoreError::QueryError(_) => "QueryError",
            CoreError::RuntimeError(_) => "RuntimeError",
            CoreError::ResourceLimitExceeded(_) => "ResourceLimitExceeded",
            CoreError::SyncError(_) => "SyncError",
            CoreError::ConflictRequiresUser(_) => "ConflictRequiresUser",
            CoreError::ProviderError(_) => "ProviderError",
            CoreError::PlatformUnavailable(_) => "PlatformUnavailable",
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Who is making a request. prd-merged/01 CR-A3 / prd-merged/03 SS-7 — every
/// command carries this and passes RBAC/capability validation before touching
/// state. In the M0a spine RBAC is minimal (single owner actor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorContext {
    pub actor: ActorId,
    pub role: Role,
}

impl ActorContext {
    pub fn owner(actor: impl Into<String>) -> Self {
        ActorContext { actor: ActorId::new(actor), role: Role::Owner }
    }
}

/// Customizable RBAC defaults. prd-merged/03 SS-7, prd-merged/07 SC-11.
/// M0a only needs the variants to exist; enforcement is owner-permits-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Maintainer,
    Editor,
    Runner,
    Viewer,
    Auditor,
    Reviewer,
}

/// Monotone logical timestamp (lamport-style). prd-merged/04 P-04 `CoreEvent`.
/// Distinct from wall-clock; the spine is deterministic so events order by this.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct LogicalTimestamp(pub u64);

impl LogicalTimestamp {
    pub fn next(self) -> Self {
        LogicalTimestamp(self.0 + 1)
    }
}

impl fmt::Display for LogicalTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A command issued by a shell (or the CLI harness) to the core.
/// prd-merged/01 CR-A1/CR-A2, prd-merged/04 P-04.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCommand {
    pub request_id: RequestId,
    pub actor: ActorContext,
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applet_id: Option<AppletId>,
    /// Command name, e.g. `applet.install`, `runtime.run`, `query.execute`.
    pub name: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// The core's reply to a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreResponse {
    pub request_id: RequestId,
    pub ok: bool,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CoreError>,
}

impl CoreResponse {
    pub fn ok(request_id: RequestId, payload: serde_json::Value) -> Self {
        CoreResponse { request_id, ok: true, payload, warnings: vec![], error: None }
    }

    pub fn err(request_id: RequestId, error: CoreError) -> Self {
        CoreResponse {
            request_id,
            ok: false,
            payload: serde_json::Value::Null,
            warnings: vec![],
            error: Some(error),
        }
    }
}

/// An event emitted by the core onto the event/stream channel.
/// prd-merged/01 CR-A1, prd-merged/02 §observability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreEvent {
    pub event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applet_id: Option<AppletId>,
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    pub created_at_logical: LogicalTimestamp,
}

/// Result returned by a script entrypoint `main(ctx, input)`.
/// prd-merged/01 CR-8 (P-07 entrypoint contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppResult {
    pub ok: bool,
    #[serde(default)]
    pub value: serde_json::Value,
}

/// Workspace settings/theme tokens visible to the UI layer. Minimal for M0a.
pub type ThemeTokens = BTreeMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_roundtrips_through_json_with_stable_code() {
        let e = CoreError::ResourceLimitExceeded("cpu budget".into());
        let s = serde_json::to_string(&e).unwrap();
        let back: CoreError = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert_eq!(back.code(), "ResourceLimitExceeded");
    }

    #[test]
    fn logical_timestamp_is_monotone_and_ordered() {
        let a = LogicalTimestamp::default();
        let b = a.next();
        assert!(b > a);
        assert_eq!(b.0, 1);
    }

    #[test]
    fn response_helpers_set_ok_flag() {
        let rid = RequestId::new("r1");
        assert!(CoreResponse::ok(rid.clone(), serde_json::json!({"x": 1})).ok);
        assert!(!CoreResponse::err(rid, CoreError::QueryError("bad".into())).ok);
    }

    #[test]
    fn command_omits_none_applet_id_in_json() {
        let cmd = CoreCommand {
            request_id: RequestId::new("r1"),
            actor: ActorContext::owner("dev"),
            workspace_id: WorkspaceId::new("ws1"),
            applet_id: None,
            name: "workspace.open".into(),
            payload: serde_json::Value::Null,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(!s.contains("applet_id"), "None applet_id should be skipped: {s}");
    }
}
