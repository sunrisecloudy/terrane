//! Runtime bridge envelope / permission / rate-budget / storage-prefix gate.
//!
//! Pure decision logic for `bridge.validate_envelope` (C10). Shells supply
//! observed bridge-call counts; the core never reads `platform.sqlite` directly.

use forge_domain::{WebappManifest, WebappResourceBudget};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const RUNTIME_ENVELOPE_FIELDS: &[&str] = &["appId", "mountToken", "request"];
const BRIDGE_REQUEST_FIELDS: &[&str] = &["id", "method", "params", "timestamp"];

/// Counts the shell observed over the last 60s (supplied to the core).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BridgeCallCounts {
    #[serde(default)]
    pub total_last_60s: u64,
    #[serde(default)]
    pub network_last_60s: u64,
    #[serde(default)]
    pub app_log_last_60s: u64,
}

/// Input to the bridge envelope gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeEnvelopeRequest {
    /// Full runtime envelope or flat bridge request object.
    pub envelope: Value,
    #[serde(default = "default_true")]
    pub is_main_frame: bool,
    pub app_id: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub resource_budget: WebappResourceBudget,
    #[serde(default)]
    pub storage_prefix: Option<String>,
    #[serde(default)]
    pub counts: BridgeCallCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_key: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Outcome returned to shells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeEnvelopeDecision {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_required: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_prefix: Option<String>,
    #[serde(default)]
    pub quarantine_eligible: bool,
}

impl BridgeEnvelopeDecision {
    fn allow(
        request_id: Option<String>,
        method: String,
        storage_prefix: String,
    ) -> Self {
        BridgeEnvelopeDecision {
            allowed: true,
            error_code: None,
            message: None,
            details: None,
            request_id,
            method: Some(method),
            permission_required: None,
            storage_prefix: Some(storage_prefix),
            quarantine_eligible: false,
        }
    }

    fn deny(
        request_id: Option<String>,
        code: &str,
        message: impl Into<String>,
        details: Value,
        quarantine_eligible: bool,
    ) -> Self {
        BridgeEnvelopeDecision {
            allowed: false,
            error_code: Some(code.into()),
            message: Some(message.into()),
            details: Some(details),
            request_id,
            method: None,
            permission_required: None,
            storage_prefix: None,
            quarantine_eligible,
        }
    }
}

/// Validate a runtime bridge envelope + permission + budget decision.
pub fn validate_bridge_envelope(input: &BridgeEnvelopeRequest) -> BridgeEnvelopeDecision {
    let envelope_obj = match input.envelope.as_object() {
        Some(obj) => obj,
        None => {
            return BridgeEnvelopeDecision::deny(
                None,
                "invalid_request",
                "Bridge message body must be an object",
                Value::Null,
                false,
            );
        }
    };

    let is_runtime = envelope_obj.contains_key("request")
        || envelope_obj.contains_key("mountToken")
        || envelope_obj.contains_key("appId");

    if is_runtime && !input.is_main_frame {
        return BridgeEnvelopeDecision::deny(
            envelope_obj.get("request").and_then(request_id),
            "bridge.unauthorized_channel",
            "Runtime bridge envelope must come from the main runtime frame",
            Value::Null,
            false,
        );
    }

    if is_runtime {
        if let Some(extra) = extra_fields(envelope_obj, RUNTIME_ENVELOPE_FIELDS) {
            return BridgeEnvelopeDecision::deny(
                envelope_obj.get("request").and_then(request_id),
                "invalid_request",
                "Runtime bridge envelope contains unknown top-level fields",
                serde_json::json!({ "fields": extra }),
                false,
            );
        }
        let app_id = envelope_obj.get("appId").and_then(|v| v.as_str()).unwrap_or("");
        let mount = envelope_obj.get("mountToken").and_then(|v| v.as_str()).unwrap_or("");
        let has_request = envelope_obj.get("request").is_some();
        if app_id.is_empty() || mount.is_empty() || !has_request {
            return BridgeEnvelopeDecision::deny(
                envelope_obj.get("request").and_then(request_id),
                "invalid_request",
                "Runtime bridge envelope requires appId, mountToken, and request",
                Value::Null,
                false,
            );
        }
    }

    let request_body = envelope_obj
        .get("request")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_else(|| envelope_obj.clone());

    if let Some(failure) = validate_bridge_request_shape(&request_body) {
        return failure;
    }

    let request_id = request_body.get("id").and_then(|v| v.as_str()).map(str::to_string);
    let method = request_body
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let params = request_body.get("params").cloned().unwrap_or(Value::Null);

    if params.as_object().map(|o| o.contains_key("appId")).unwrap_or(false) {
        return BridgeEnvelopeDecision::deny(
            request_id,
            "invalid_request",
            "Bridge params must not include appId; app id is channel-derived",
            serde_json::json!({ "field": "appId" }),
            false,
        );
    }

    let storage_prefix = input
        .storage_prefix
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| format!("{}:", input.app_id));

    if let Some(required) = permission_for_bridge_method(&method) {
        let approved: BTreeSet<String> = input.permissions.iter().cloned().collect();
        if !approved.contains(&required) {
            return BridgeEnvelopeDecision {
                allowed: false,
                error_code: Some("permission_denied".into()),
                message: Some(format!(
                    "App {} cannot call {}",
                    input.app_id, method
                )),
                details: Some(serde_json::json!({
                    "appId": input.app_id,
                    "method": method,
                    "requiredPermission": required,
                })),
                request_id,
                method: Some(method),
                permission_required: Some(required),
                storage_prefix: Some(storage_prefix),
                quarantine_eligible: false,
            };
        }
    }

    if let Some(budget_failure) = rate_budget_failure(&method, &params, &input.resource_budget, &input.counts) {
        return BridgeEnvelopeDecision {
            allowed: false,
            error_code: Some("resource_budget_exceeded".into()),
            message: Some(budget_failure.message),
            details: Some(budget_failure.details),
            request_id,
            method: Some(method.clone()),
            permission_required: None,
            storage_prefix: Some(storage_prefix.clone()),
            quarantine_eligible: true,
        };
    }

    if method.starts_with("storage.") {
        if let Some(key) = &input.storage_key {
            if !key.starts_with(&storage_prefix) {
                return BridgeEnvelopeDecision::deny(
                    request_id,
                    "permission_denied",
                    format!("Storage key must begin with {storage_prefix}"),
                    serde_json::json!({
                        "key": key,
                        "prefix": storage_prefix,
                        "appId": input.app_id,
                    }),
                    false,
                );
            }
        }
    }

    BridgeEnvelopeDecision::allow(request_id, method, storage_prefix)
}

/// Map a bridge method to its manifest permission token.
pub fn permission_for_bridge_method(method: &str) -> Option<String> {
    match method {
        "storage.get" | "storage.list" => Some("storage.read".into()),
        "storage.set" | "storage.remove" => Some("storage.write".into()),
        "dialog.openFile" | "dialog.saveFile" | "notification.toast" | "network.request"
        | "core.step" => Some(method.to_string()),
        _ => None,
    }
}

/// Derive sandbox fields from a validated manifest.
pub fn bridge_context_from_manifest(manifest: &WebappManifest) -> (Vec<String>, String, WebappResourceBudget) {
    (
        manifest.permissions.clone(),
        manifest.effective_storage_prefix(),
        manifest.resource_budget.clone(),
    )
}

struct BudgetFailure {
    message: String,
    details: Value,
}

fn rate_budget_failure(
    method: &str,
    params: &Value,
    budget: &WebappResourceBudget,
    counts: &BridgeCallCounts,
) -> Option<BudgetFailure> {
    if let Some(limit) = budget.max_bridge_calls_per_minute {
        if counts.total_last_60s >= limit {
            return Some(BudgetFailure {
                message: "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute".into(),
                details: serde_json::json!({
                    "budget": "maxBridgeCallsPerMinute",
                    "current": counts.total_last_60s,
                    "max": limit,
                    "limit": limit,
                }),
            });
        }
    }
    if method == "network.request" {
        if let Some(limit) = budget.max_network_requests_per_minute {
            if counts.network_last_60s >= limit {
                return Some(BudgetFailure {
                    message: "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute".into(),
                    details: serde_json::json!({
                        "budget": "maxNetworkRequestsPerMinute",
                        "current": counts.network_last_60s,
                        "max": limit,
                        "limit": limit,
                    }),
                });
            }
        }
    }
    if method == "app.log" {
        if let Some(limit) = budget.max_log_lines_per_minute {
            if counts.app_log_last_60s >= limit {
                return Some(BudgetFailure {
                    message: "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute".into(),
                    details: serde_json::json!({
                        "budget": "maxLogLinesPerMinute",
                        "current": counts.app_log_last_60s,
                        "max": limit,
                        "limit": limit,
                    }),
                });
            }
        }
        if let Some(level) = params.get("level").and_then(|v| v.as_str()) {
            if !["debug", "info", "warn", "error"].contains(&level) {
                return Some(BudgetFailure {
                    message: "app.log level must be debug, info, warn, or error".into(),
                    details: serde_json::json!({ "level": level }),
                });
            }
        }
        if params
            .get("message")
            .and_then(|v| v.as_str())
            .map(|s| s.is_empty())
            .unwrap_or(true)
        {
            return Some(BudgetFailure {
                message: "app.log requires message".into(),
                details: Value::Null,
            });
        }
    }
    None
}

fn validate_bridge_request_shape(body: &serde_json::Map<String, Value>) -> Option<BridgeEnvelopeDecision> {
    if let Some(extra) = extra_fields(body, BRIDGE_REQUEST_FIELDS) {
        return Some(BridgeEnvelopeDecision::deny(
            body.get("id").and_then(|v| v.as_str()).map(str::to_string),
            "invalid_request",
            "Bridge request contains unknown top-level fields",
            serde_json::json!({ "fields": extra }),
            false,
        ));
    }
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return Some(BridgeEnvelopeDecision::deny(
            None,
            "invalid_request",
            "Bridge request id must be a non-empty string",
            Value::Null,
            false,
        ));
    }
    if let Some(ts) = body.get("timestamp") {
        if !is_finite_json_number(ts) {
            return Some(BridgeEnvelopeDecision::deny(
                Some(id.into()),
                "invalid_request",
                "Bridge request timestamp must be a finite number",
                Value::Null,
                false,
            ));
        }
    }
    if !body.get("method").map(|v| v.is_string()).unwrap_or(false) {
        return Some(BridgeEnvelopeDecision::deny(
            Some(id.into()),
            "invalid_request",
            "Bridge request method must be a string",
            Value::Null,
            false,
        ));
    }
    if !body.get("params").map(|v| v.is_object()).unwrap_or(false) {
        return Some(BridgeEnvelopeDecision::deny(
            Some(id.into()),
            "invalid_request",
            "Bridge request params must be an object",
            Value::Null,
            false,
        ));
    }
    None
}

fn extra_fields(map: &serde_json::Map<String, Value>, allowed: &[&str]) -> Option<Vec<String>> {
    let allowed: BTreeSet<&str> = allowed.iter().copied().collect();
    let extra: Vec<String> = map
        .keys()
        .filter(|k| !allowed.contains(k.as_str()))
        .cloned()
        .collect();
    if extra.is_empty() { None } else { Some(extra) }
}

fn request_id(value: &Value) -> Option<String> {
    value
        .as_object()
        .and_then(|o| o.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn is_finite_json_number(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_f64().map(|f| f.is_finite()).unwrap_or(false),
        Value::Bool(_) => false,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_input() -> BridgeEnvelopeRequest {
        BridgeEnvelopeRequest {
            envelope: json!({
                "appId": "notes-lite",
                "mountToken": "tok",
                "request": {
                    "id": "req1",
                    "method": "storage.get",
                    "params": { "key": "notes-lite:note/1" }
                }
            }),
            is_main_frame: true,
            app_id: "notes-lite".into(),
            permissions: vec!["storage.read".into()],
            resource_budget: WebappResourceBudget::default(),
            storage_prefix: Some("notes-lite:".into()),
            counts: BridgeCallCounts::default(),
            storage_key: Some("notes-lite:note/1".into()),
        }
    }

    #[test]
    fn valid_storage_get_allowed() {
        let decision = validate_bridge_envelope(&base_input());
        assert!(decision.allowed, "{decision:?}");
    }

    #[test]
    fn missing_permission_denied() {
        let mut input = base_input();
        input.permissions.clear();
        let decision = validate_bridge_envelope(&input);
        assert!(!decision.allowed);
        assert_eq!(decision.error_code.as_deref(), Some("permission_denied"));
    }

    #[test]
    fn storage_prefix_enforced() {
        let mut input = base_input();
        input.storage_key = Some("other:note/1".into());
        let decision = validate_bridge_envelope(&input);
        assert!(!decision.allowed);
    }

    #[test]
    fn bridge_budget_exceeded_is_quarantine_eligible() {
        let mut input = base_input();
        input.resource_budget.max_bridge_calls_per_minute = Some(1);
        input.counts.total_last_60s = 1;
        let decision = validate_bridge_envelope(&input);
        assert!(!decision.allowed);
        assert!(decision.quarantine_eligible);
    }
}