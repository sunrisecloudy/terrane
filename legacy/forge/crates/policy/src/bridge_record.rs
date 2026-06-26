//! Deterministic bridge / core-event / session record IDs (C11).
//!
//! Pure logic: shells still perform the SQLite INSERT, but IDs and normalized
//! row shapes are core-owned so replay captures the same audit schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Platform + target pair that namespaces deterministic IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgePlatformIds {
    pub platform: String,
    pub target: String,
}

/// Deterministic runtime session id for a mount.
pub fn runtime_session_id(ids: &BridgePlatformIds, app_id: &str, mount_token: &str) -> String {
    let mount = sanitize_token(mount_token);
    format!("runtime_{}_{}_{}_{}", ids.platform, ids.target, sanitize_token(app_id), mount)
}

/// Deterministic bridge-call id from the client request id.
pub fn bridge_call_id(ids: &BridgePlatformIds, request_id: &str) -> String {
    format!(
        "bridge_{}_{}_{}",
        ids.platform,
        ids.target,
        sanitize_token(request_id)
    )
}

/// Deterministic core-event id.
pub fn core_event_id(ids: &BridgePlatformIds, request_id: &str) -> String {
    format!(
        "core_event_{}_{}_{}",
        ids.platform,
        ids.target,
        sanitize_token(request_id)
    )
}

/// Deterministic core-action id.
pub fn core_action_id(ids: &BridgePlatformIds, request_id: &str, action_index: usize) -> String {
    format!(
        "core_action_{}_{}_{}_{action_index}",
        ids.platform,
        ids.target,
        sanitize_token(request_id)
    )
}

/// Normalized `runtime_sessions` metadata for crash recovery (replay inputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionMetadata {
    pub source: String,
    #[serde(default)]
    pub reload_offered: bool,
    #[serde(default)]
    pub can_auto_remount: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub runtime_ready: bool,
}

impl RuntimeSessionMetadata {
    pub fn running_webview(source: impl Into<String>) -> Self {
        RuntimeSessionMetadata {
            source: source.into(),
            reload_offered: false,
            can_auto_remount: false,
            reason: None,
            runtime_ready: false,
        }
    }

    pub fn terminated(source: impl Into<String>, can_auto_remount: bool) -> Self {
        RuntimeSessionMetadata {
            source: source.into(),
            reload_offered: true,
            can_auto_remount,
            reason: Some("web_content_process_terminated".into()),
            runtime_ready: false,
        }
    }
}

/// Row the shell inserts into `bridge_calls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeCallRecord {
    pub bridge_call_id: String,
    pub session_id: String,
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub method: String,
    pub params_json: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_json: Option<Value>,
    pub duration_ms: i64,
}

/// Row the shell inserts into `core_events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreEventRecord {
    pub event_id: String,
    pub session_id: String,
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_version_before: Option<i64>,
    pub event_json: Value,
}

/// Row the shell inserts into `core_actions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreActionRecord {
    pub action_id: String,
    pub event_id: String,
    pub session_id: String,
    pub app_id: String,
    pub action_json: Value,
}

pub fn state_version_before(result_state_version: Option<i64>) -> Option<i64> {
    result_state_version.map(|v| v.saturating_sub(1).max(0))
}

fn sanitize_token(token: &str) -> String {
    let mut out = String::new();
    for ch in token.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> BridgePlatformIds {
        BridgePlatformIds {
            platform: "macos".into(),
            target: "native".into(),
        }
    }

    #[test]
    fn ids_are_deterministic_and_namespaced() {
        let ids = ids();
        assert_eq!(
            bridge_call_id(&ids, "req_storage_get"),
            "bridge_macos_native_req_storage_get"
        );
        assert_eq!(
            core_event_id(&ids, "req_core_step"),
            "core_event_macos_native_req_core_step"
        );
        assert_eq!(
            runtime_session_id(&ids, "notes-lite", "mount-1"),
            "runtime_macos_native_notes-lite_mount-1"
        );
    }

    #[test]
    fn crash_metadata_captures_reload_inputs() {
        let meta = RuntimeSessionMetadata::terminated("native-macos-webview", true);
        assert!(meta.reload_offered);
        assert!(meta.can_auto_remount);
        assert_eq!(meta.reason.as_deref(), Some("web_content_process_terminated"));
    }
}