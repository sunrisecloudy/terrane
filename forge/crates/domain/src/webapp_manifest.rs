//! Legacy generated-webapp manifest (`schemas/manifest.schema.json`).
//!
//! Distinct from the v1 workspace [`Manifest`](crate::Manifest): this is the
//! `platform.sqlite` / `app_versions.manifest_json` shape shells use for bridge
//! sandbox decisions (Q8 `package.*` namespace).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Full trusted webapp manifest deserialized from `manifest_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebappManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime_version: String,
    pub data_version: i64,
    pub entry: String,
    pub description: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "default_storage_prefix")]
    pub storage_prefix: String,
    #[serde(default)]
    pub network_policy: WebappNetworkPolicy,
    #[serde(default)]
    pub resource_budget: WebappResourceBudget,
    #[serde(default)]
    pub capabilities: WebappCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_rating: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_capabilities: Option<Vec<String>>,
}

fn default_storage_prefix() -> String {
    String::new()
}

/// `manifest.networkPolicy` (docs/24 / `schemas/network-policy.schema.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebappNetworkPolicy {
    #[serde(default)]
    pub allow: Vec<WebappNetworkAllowEntry>,
    /// Defaults to `true` when absent (docs/24).
    #[serde(default = "default_deny_private_network")]
    pub deny_private_network: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_credentials: Option<bool>,
}

fn default_deny_private_network() -> bool {
    true
}

/// One `networkPolicy.allow[]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebappNetworkAllowEntry {
    pub origin: String,
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `manifest.resourceBudget` subset used by bridge gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebappResourceBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bridge_calls_per_minute: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_network_requests_per_minute: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_log_lines_per_minute: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_network_response_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_storage_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dom_nodes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_timers: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_package_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
}

/// Declared capability buckets (informational; permissions drive bridge gates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebappCapabilities {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

/// Trusted sandbox view derived from a validated manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebappSandboxContext {
    pub app_id: String,
    pub storage_prefix: String,
    pub permissions: Vec<String>,
    pub network_policy: WebappNetworkPolicy,
    pub deny_private_network: bool,
    pub resource_budget: WebappResourceBudget,
}

impl WebappManifest {
    /// Parse and validate a trusted `manifest_json` blob from `app_versions`.
    pub fn from_json_value(value: &serde_json::Value) -> crate::Result<Self> {
        let manifest: WebappManifest = serde_json::from_value(value.clone()).map_err(|e| {
            crate::CoreError::ValidationError(format!("manifest_json is malformed: {e}"))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.id.trim().is_empty() {
            return Err(crate::CoreError::ValidationError("manifest.id is empty".into()));
        }
        if self.entry.trim().is_empty() {
            return Err(crate::CoreError::ValidationError("manifest.entry is empty".into()));
        }
        let prefix = self.effective_storage_prefix();
        if !prefix.ends_with(':') {
            return Err(crate::CoreError::ValidationError(format!(
                "manifest.storagePrefix must end with ':', got {prefix:?}"
            )));
        }
        for perm in &self.permissions {
            if perm.trim().is_empty() {
                return Err(crate::CoreError::ValidationError(
                    "manifest.permissions contains an empty entry".into(),
                ));
            }
        }
        self.network_policy.validate()?;
        Ok(())
    }

    pub fn effective_storage_prefix(&self) -> String {
        if !self.storage_prefix.is_empty() {
            return self.storage_prefix.clone();
        }
        format!("{}:", self.id)
    }

    pub fn permissions_set(&self) -> BTreeSet<String> {
        self.permissions.iter().cloned().collect()
    }

    pub fn sandbox_context(&self) -> WebappSandboxContext {
        WebappSandboxContext {
            app_id: self.id.clone(),
            storage_prefix: self.effective_storage_prefix(),
            permissions: self.permissions.clone(),
            network_policy: self.network_policy.clone(),
            deny_private_network: self.network_policy.deny_private_network,
            resource_budget: self.resource_budget.clone(),
        }
    }
}

impl WebappNetworkPolicy {
    pub fn validate(&self) -> crate::Result<()> {
        if self.allow_credentials == Some(true) {
            return Err(crate::CoreError::ValidationError(
                "manifest.networkPolicy.allowCredentials must be false in v0.4".into(),
            ));
        }
        for entry in &self.allow {
            entry.validate()?;
        }
        Ok(())
    }
}

impl WebappNetworkAllowEntry {
    pub fn matches_target(&self, origin: &str, method: &str, path: &str) -> bool {
        if self.origin != origin {
            return false;
        }
        if !self.methods.iter().any(|m| m.eq_ignore_ascii_case(method)) {
            return false;
        }
        if let Some(prefix) = &self.path_prefix {
            if !path.starts_with(prefix) {
                return false;
            }
        }
        true
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.origin.trim().is_empty() {
            return Err(crate::CoreError::ValidationError(
                "networkPolicy.allow[].origin is empty".into(),
            ));
        }
        if self.methods.is_empty() {
            return Err(crate::CoreError::ValidationError(
                "networkPolicy.allow[].methods must be non-empty".into(),
            ));
        }
        if let Some(ms) = self.timeout_ms {
            if ms == 0 || ms > 120_000 {
                return Err(crate::CoreError::ValidationError(format!(
                    "networkPolicy.timeoutMs must be from 1 to 120000, got {ms}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_dashboard_manifest() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../webapps/examples/api-dashboard/manifest.json"
        ))
        .unwrap()
    }

    #[test]
    fn parses_example_manifest() {
        let manifest = WebappManifest::from_json_value(&api_dashboard_manifest()).unwrap();
        assert_eq!(manifest.id, "api-dashboard");
        assert!(manifest.permissions.contains(&"network.request".to_string()));
        assert_eq!(manifest.effective_storage_prefix(), "api-dashboard:");
        assert!(manifest.network_policy.deny_private_network);
    }

    #[test]
    fn permissions_roundtrip() {
        let manifest = WebappManifest::from_json_value(&api_dashboard_manifest()).unwrap();
        let ctx = manifest.sandbox_context();
        assert_eq!(ctx.permissions, manifest.permissions);
        assert_eq!(ctx.storage_prefix, "api-dashboard:");
    }

    #[test]
    fn allow_credentials_true_is_rejected() {
        let mut value = api_dashboard_manifest();
        value["networkPolicy"]["allowCredentials"] = serde_json::json!(true);
        assert_eq!(
            WebappManifest::from_json_value(&value).unwrap_err().code(),
            "ValidationError"
        );
    }
}