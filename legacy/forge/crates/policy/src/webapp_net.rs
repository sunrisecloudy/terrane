//! Webapp `manifest.networkPolicy` egress gate (docs/24).
//!
//! This is the legacy generated-app allowlist shape (`origin` + `methods` +
//! `pathPrefix` + header/body caps), distinct from the v1 workspace
//! [`NetPolicy`](crate::NetPolicy) (`capabilities.net` URL rules). Shells
//! delegate `network.request` preflight here via `bridge.validate_network_request`.

use crate::net_url::{host_is_private_literal, ParsedUrl};
use forge_domain::{is_supported_net_scheme, WebappNetworkAllowEntry, WebappNetworkPolicy, WebappResourceBudget};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A `network.request` the shell is about to perform (request-side preflight).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebappNetRequest {
    pub url: String,
    #[serde(default = "default_get")]
    pub method: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional redirect target to re-check (response-leg / redirect guard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_url: Option<String>,
    /// Optional response body size for response-leg checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
}

fn default_get() -> String {
    "GET".into()
}

impl Default for WebappNetRequest {
    fn default() -> Self {
        WebappNetRequest {
            url: String::new(),
            method: default_get(),
            headers: BTreeMap::new(),
            body_bytes: None,
            credentials: None,
            timeout_ms: None,
            redirect_url: None,
            response_bytes: None,
        }
    }
}

/// Outcome of a webapp network-policy decision for shells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebappNetDecision {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl WebappNetDecision {
    pub fn allow() -> Self {
        WebappNetDecision { allowed: true, error_code: None, message: None, details: None }
    }

    pub fn deny(code: &str, message: impl Into<String>, details: serde_json::Value) -> Self {
        WebappNetDecision {
            allowed: false,
            error_code: Some(code.into()),
            message: Some(message.into()),
            details: Some(details),
        }
    }
}

/// Decide whether `request` is permitted by `policy` (and optional `budget`).
pub fn check_webapp_network(
    policy: &WebappNetworkPolicy,
    request: &WebappNetRequest,
    budget: Option<&WebappResourceBudget>,
) -> WebappNetDecision {
    match check_webapp_network_result(policy, request, budget) {
        Ok(()) => WebappNetDecision::allow(),
        Err(decision) => decision,
    }
}

fn check_webapp_network_result(
    policy: &WebappNetworkPolicy,
    request: &WebappNetRequest,
    budget: Option<&WebappResourceBudget>,
) -> std::result::Result<(), WebappNetDecision> {
    let target = match ParsedUrl::parse(&request.url) {
        Ok(url) => url,
        Err(e) => {
            return Err(WebappNetDecision::deny(
                "invalid_request",
                format!("network.request requires an absolute url: {e}"),
                serde_json::json!({ "url": request.url }),
            ));
        }
    };
    if !is_supported_net_scheme(&target.scheme) {
        return Err(WebappNetDecision::deny(
            "invalid_request",
            "network.request url must use http or https",
            serde_json::json!({ "url": request.url }),
        ));
    }
    let origin = match origin_for(&target) {
        Some(origin) => origin,
        None => {
            return Err(WebappNetDecision::deny(
                "invalid_request",
                "network.request requires an absolute url",
                serde_json::json!({ "url": request.url }),
            ));
        }
    };

    if policy.deny_private_network && host_is_private_literal(&target.host) {
        return Err(WebappNetDecision::deny(
            "network_policy_denied",
            "network.request private network targets are denied",
            serde_json::json!({ "origin": origin, "host": target.host }),
        ));
    }

    let method = request.method.to_ascii_uppercase();
    let path = if target.path.is_empty() { "/" } else { &target.path };

    let rule = policy
        .allow
        .iter()
        .find(|entry| entry.matches_target(&origin, &method, path));

    let Some(rule) = rule else {
        return Err(WebappNetDecision::deny(
            "network_policy_denied",
            "network.request is outside manifest.networkPolicy",
            serde_json::json!({ "origin": origin, "method": method }),
        ));
    };

    if let Some(violation) = header_violation(&request.headers, rule) {
        return Err(WebappNetDecision::deny(
            "network_policy_denied",
            if violation.credential {
                "network.request credential headers are not allowed"
            } else {
                "network.request header is outside manifest.networkPolicy"
            },
            violation.details,
        ));
    }

    if request.credentials.is_some() && !request.credentials.as_ref().map(|v| v.is_null()).unwrap_or(true) {
        return Err(WebappNetDecision::deny(
            "network_policy_denied",
            "network.request credentials are not allowed",
            serde_json::json!({ "credentials": request.credentials }),
        ));
    }

    if let (Some(max), Some(actual)) = (rule.max_request_bytes, request.body_bytes) {
        if actual > max {
            return Err(WebappNetDecision::deny(
                "network_policy_denied",
                "network.request body exceeds manifest.networkPolicy maxRequestBytes",
                serde_json::json!({ "maxRequestBytes": max, "bytes": actual }),
            ));
        }
    }

    if let Some(ms) = request.timeout_ms {
        if ms == 0 {
            return Err(WebappNetDecision::deny(
                "invalid_request",
                "network.request timeoutMs must be a positive integer",
                serde_json::json!({ "timeoutMs": ms }),
            ));
        }
        if let Some(rule_ms) = rule.timeout_ms {
            if ms > rule_ms {
                return Err(WebappNetDecision::deny(
                    "timeout",
                    "network.request timed out",
                    serde_json::json!({ "timeoutMs": rule_ms.min(ms) }),
                ));
            }
        }
    }

    if let Some(bytes) = request.response_bytes {
        let limit = effective_max_response_bytes(rule, budget);
        if let Some(max) = limit {
            if bytes > max {
                return Err(WebappNetDecision::deny(
                    "network_policy_denied",
                    "network.response exceeds manifest.networkPolicy maxResponseBytes",
                    serde_json::json!({ "maxResponseBytes": max, "bytes": bytes }),
                ));
            }
        }
    }

    if let Some(redirect) = &request.redirect_url {
        let redirect_req = WebappNetRequest {
            url: redirect.clone(),
            method: method.clone(),
            headers: request.headers.clone(),
            body_bytes: None,
            credentials: None,
            timeout_ms: None,
            redirect_url: None,
            response_bytes: None,
        };
        check_webapp_network_result(policy, &redirect_req, budget)?;
    }

    Ok(())
}

struct HeaderViolation {
    credential: bool,
    details: serde_json::Value,
}

fn header_violation(headers: &BTreeMap<String, String>, rule: &WebappNetworkAllowEntry) -> Option<HeaderViolation> {
    let allowed: std::collections::BTreeSet<String> = rule
        .allowed_headers
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    for name in headers.keys() {
        let normalized = name.to_ascii_lowercase();
        if normalized == "cookie" || normalized == "set-cookie" {
            return Some(HeaderViolation {
                credential: true,
                details: serde_json::json!({ "header": name }),
            });
        }
        if !allowed.contains(&normalized) {
            return Some(HeaderViolation {
                credential: false,
                details: serde_json::json!({
                    "header": name,
                    "allowedHeaders": allowed.iter().collect::<Vec<_>>(),
                }),
            });
        }
    }
    None
}

fn effective_max_response_bytes(
    rule: &WebappNetworkAllowEntry,
    budget: Option<&WebappResourceBudget>,
) -> Option<u64> {
    let policy_limit = rule.max_response_bytes;
    let budget_limit = budget.and_then(|b| b.max_network_response_bytes);
    match (policy_limit, budget_limit) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn origin_for(url: &ParsedUrl) -> Option<String> {
    if !is_supported_net_scheme(&url.scheme) {
        return None;
    }
    Some(format!("{}://{}", url.scheme, url.host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::WebappManifest;

    fn api_policy() -> WebappNetworkPolicy {
        WebappManifest::from_json_value(&serde_json::from_str(include_str!(
            "../../../../webapps/examples/api-dashboard/manifest.json"
        ))
        .unwrap())
        .unwrap()
        .network_policy
    }

    fn req(url: &str) -> WebappNetRequest {
        WebappNetRequest { url: url.into(), ..Default::default() }
    }

    #[test]
    fn allowed_origin_passes() {
        let decision = check_webapp_network(&api_policy(), &req("https://api.example.com/v1/x"), None);
        assert!(decision.allowed, "{decision:?}");
    }

    #[test]
    fn private_host_denied_by_default() {
        let decision = check_webapp_network(&api_policy(), &req("http://127.0.0.1/status"), None);
        assert!(!decision.allowed);
        assert_eq!(decision.error_code.as_deref(), Some("network_policy_denied"));
    }

    #[test]
    fn unlisted_origin_denied() {
        let decision = check_webapp_network(
            &api_policy(),
            &req("https://evil.example.net/secret"),
            None,
        );
        assert!(!decision.allowed);
    }

    #[test]
    fn path_prefix_denied() {
        let mut policy = api_policy();
        policy.allow[0].path_prefix = Some("/public".into());
        let decision = check_webapp_network(&policy, &req("https://api.example.com/v1/x"), None);
        assert!(!decision.allowed);
    }

    #[test]
    fn credential_header_denied() {
        let mut request = req("https://api.example.com/v1/x");
        request.headers.insert("Cookie".into(), "a=b".into());
        let decision = check_webapp_network(&api_policy(), &request, None);
        assert!(!decision.allowed);
    }

    #[test]
    fn redirect_rechecked() {
        let mut request = req("https://api.example.com/v1/x");
        request.redirect_url = Some("http://127.0.0.1/admin".into());
        let decision = check_webapp_network(&api_policy(), &request, None);
        assert!(!decision.allowed);
    }
}