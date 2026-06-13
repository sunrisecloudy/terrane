//! `NetPolicy`: the network egress allow/deny engine for `ctx.net.fetch`.
//!
//! Normative spec: prd-merged/07 **SC-5** (network egress policy), **SC-8** (net
//! capability grammar), prd-merged/01 **CR-3** (`net` namespace). Rules ported
//! from `docs/24_NETWORK_POLICY.md` — the *rules*, not the v0.4 manifest shape:
//!   - host must be an **exact literal** (SC-8: no wildcard hosts in v1);
//!   - `https` is required unless the rule pattern itself is `http://`
//!     (a request may not downgrade an `https` rule to `http`);
//!   - the path may carry a trailing `*` glob and must stay within it;
//!   - the method must match;
//!   - request/response body sizes must be within the rule's caps;
//!   - the request timeout must be within the rule's timeout budget;
//!   - request/response content-types, if the rule constrains them, must match;
//!   - **private-network targets are denied by default**: loopback, link-local,
//!     RFC1918, carrier-grade NAT, IPv6 unique-local / link-local literals, the
//!     cloud-metadata address, and the `localhost` hostname — denied *before*
//!     the allow rules, so even a rule that literally lists `localhost` cannot
//!     reach it (SC-5 `denyPrivateNetwork` defaults true).
//!
//! ## What is decided here vs. at runtime (DNS pinning honesty)
//!
//! This engine is **pure, deterministic, wasm-clean** and decides on *literal*
//! URL/host/IP text only. True DNS pinning — resolving a hostname and rejecting
//! a private *answer* — is a **runtime** concern: the resolver runs host-side,
//! not in this crate. We document that boundary explicitly:
//!   - a redirect chain whose hops are *literal* private IPs **is** caught here
//!     (every hop is re-checked, SC-5 "redirect targets are checked the same
//!     way"); a redirect to a private *hostname* that only resolves privately is
//!     a runtime concern;
//!   - `dns_answers` carrying a private literal IP **is** caught here as a
//!     best-effort literal check, but the authoritative DNS-pin/recheck is a
//!     runtime concern.
//!
//! The T011 fixtures flag the vectors that need the runtime resolver via
//! `requires_runtime_dns`; the data-driven test asserts those separately.

use crate::net_url::{host_is_private_literal, ParsedUrl};
use forge_domain::{CoreError, NetGrant, NetRule, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A concrete `ctx.net.fetch` request the runtime is about to perform, as the
/// policy engine needs to see it. Field names mirror the T011 fixtures'
/// `request` object so a fixture request deserializes straight into this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetRequest {
    /// HTTP method, e.g. `GET`, `POST`. Matched case-insensitively.
    pub method: String,
    /// Absolute request URL (`scheme://host[:port]/path`).
    pub url: String,
    /// Declared request body size in bytes (against `max_body_bytes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<u64>,
    /// Declared/expected response body size in bytes (against
    /// `max_response_bytes`). A recorded response carries its real size here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    /// Requested timeout in milliseconds (against the rule's `timeout_ms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Request `Content-Type` (against `request_content_types`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_content_type: Option<String>,
    /// Response `Content-Type` (against `response_content_types`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_content_type: Option<String>,
    /// Request headers. A value is either a literal string or an object with a
    /// `secret_ref` (a secret to be injected) — see [`HeaderValue`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, HeaderValue>,
    /// The full redirect chain (origin first, final hop last), if the request
    /// followed redirects. Every hop is re-checked. SC-5 redirect handling; a
    /// chain with a private *literal* hop is caught here, a private *resolved*
    /// hop is a runtime concern (`requires_runtime_dns`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_chain: Vec<String>,
    /// Literal DNS answers the runtime resolved for the host, if provided. A
    /// private literal answer is caught here best-effort; the authoritative
    /// DNS-pin recheck is a runtime concern (`requires_runtime_dns`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_answers: Vec<String>,
}

/// A request header value: either a plain literal string or a secret reference
/// the host injects. A `secret_ref` header may only be sent to a destination
/// whose matching rule lists the header name in `allow_secret_headers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HeaderValue {
    /// A secret reference, e.g. `{ "secret_ref": "secret_weather" }`.
    Secret {
        #[allow(dead_code)]
        secret_ref: String,
    },
    /// A literal header value, e.g. `"Bearer abc"`.
    Literal(String),
}

/// Header names that are treated as secret-bearing: a literal value for one of
/// these is denied (the applet must route the secret through the host's
/// secret-injection policy, not embed it). prd-merged/07 secrets handling.
fn is_secret_header_name(name: &str) -> bool {
    const SECRET_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];
    let lower = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&lower.as_str())
}

/// The network egress policy engine (SC-5). Built from a manifest's [`NetGrant`]
/// allowlist; [`check`](NetPolicy::check) decides one request.
#[derive(Debug, Clone)]
pub struct NetPolicy<'a> {
    allowlist: &'a NetGrant,
}

impl<'a> NetPolicy<'a> {
    /// Build a policy view over an allowlist.
    pub fn new(allowlist: &'a NetGrant) -> Self {
        NetPolicy { allowlist }
    }

    /// Decide whether `request` is permitted by the allowlist (SC-5).
    ///
    /// Returns:
    ///   - `Ok(())` if the request matches a rule and passes every SC-5 check;
    ///   - `Err(CapabilityRequired)` if the allowlist is **empty** — the applet
    ///     never requested the `net` capability at all (mirrors storage/db
    ///     "category not declared");
    ///   - `Err(PermissionDenied)` if the allowlist is non-empty but the request
    ///     is blocked: a private/loopback target, a scheme/host/path/method
    ///     mismatch, a size/timeout/content-type violation, a wildcard-host rule
    ///     (rejected), or a disallowed secret header.
    ///
    /// Order: the **private-network deny** runs first on the request host and
    /// every redirect hop / DNS answer, so a private target is refused even if a
    /// rule literally names it. Then the per-rule match runs.
    pub fn check(&self, request: &NetRequest) -> Result<()> {
        // Empty allowlist ⇒ no net capability declared at all ⇒ CapabilityRequired,
        // distinct from "declared rules but none cover this request" below.
        if self.allowlist.is_empty() {
            return Err(CoreError::CapabilityRequired(format!(
                "manifest declares no net capability; cannot fetch {:?} (add a capabilities.net rule)",
                request.url
            )));
        }

        // Parse the request URL up front; a malformed URL is a flat denial.
        let target = ParsedUrl::parse(&request.url).map_err(|e| {
            CoreError::PermissionDenied(format!("net.fetch denied: invalid url {:?}: {e}", request.url))
        })?;

        // SC-5 private-network deny: the request host, every redirect hop, and
        // every literal DNS answer must not be a private/loopback/metadata
        // literal or the `localhost` hostname — denied before any allow rule.
        deny_private_target(&target.host)?;
        for hop in &request.redirect_chain {
            let hop_url = ParsedUrl::parse(hop).map_err(|e| {
                CoreError::PermissionDenied(format!(
                    "net.fetch denied: invalid redirect target {hop:?}: {e}"
                ))
            })?;
            deny_private_target(&hop_url.host)?;
        }
        for answer in &request.dns_answers {
            // A DNS answer is a bare IP literal, not a URL.
            if host_is_private_literal(answer) {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: DNS answer {answer:?} resolves to a private/loopback address (SC-5 deny private network)"
                )));
            }
        }

        // Secret-bearing literal headers are denied outright: a secret must go
        // through the host's secret-injection policy, never embedded as a literal.
        for (name, value) in &request.headers {
            if let HeaderValue::Literal(_) = value {
                if is_secret_header_name(name) {
                    return Err(CoreError::PermissionDenied(format!(
                        "net.fetch denied: secret-like header {name:?} carries a literal value; route secrets through the secret-injection policy (allow_secret_headers), not a literal"
                    )));
                }
            }
        }

        // Find the first rule that matches host+scheme+method+path. If none
        // matches we collect why and surface a PermissionDenied (the capability
        // IS declared, this request just isn't covered).
        let mut last_reason: Option<CoreError> = None;
        for rule in self.allowlist.rules() {
            match self.rule_matches(rule, request, &target) {
                Ok(()) => return Ok(()),
                Err(reason) => last_reason = Some(reason),
            }
        }
        Err(last_reason.unwrap_or_else(|| {
            CoreError::PermissionDenied(format!(
                "net.fetch denied: no net rule covers {} {:?}",
                request.method, request.url
            ))
        }))
    }

    /// Check a single rule against the request. `Ok` if this rule fully grants
    /// the request; `Err` names the first constraint this rule fails (used as
    /// the denial reason when no rule matches).
    fn rule_matches(&self, rule: &NetRule, request: &NetRequest, target: &ParsedUrl) -> Result<()> {
        let pattern = ParsedUrl::parse(&rule.url).map_err(|e| {
            CoreError::PermissionDenied(format!(
                "net.fetch denied: malformed net rule url {:?}: {e}",
                rule.url
            ))
        })?;

        // SC-8: wildcard hosts are forbidden in v1. A rule host containing `*`
        // is invalid and matches nothing.
        if pattern.host.contains('*') {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: wildcard host {:?} is forbidden in v1 (SC-8 exact hosts only)",
                pattern.host
            )));
        }

        // Scheme: https required unless the rule itself is http. A request may
        // not satisfy an https rule over http (no downgrade), and an http rule
        // never matches an https request host pattern mismatch handled by host.
        if rule_scheme_denies(&pattern.scheme, &target.scheme) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: scheme {:?} does not satisfy rule scheme {:?} (https required, no downgrade)",
                target.scheme, pattern.scheme
            )));
        }

        // Host: exact, case-insensitive literal match (SC-8 no wildcard hosts).
        if !target.host.eq_ignore_ascii_case(&pattern.host) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: host {:?} is not the allowlisted host {:?}",
                target.host, pattern.host
            )));
        }

        // Method: case-insensitive match.
        if !request.method.eq_ignore_ascii_case(&rule.method) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: method {:?} is not granted (rule allows {:?})",
                request.method, rule.method
            )));
        }

        // Path: trailing-`*` glob or exact.
        if !path_matches(&pattern.path, &target.path) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: path {:?} is outside the allowlisted glob {:?}",
                target.path, pattern.path
            )));
        }

        // SC-5 size caps.
        if let (Some(max), Some(actual)) = (rule.max_body_bytes, request.body_bytes) {
            if actual > max {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: request body {actual} bytes exceeds max_body_bytes {max}"
                )));
            }
        }
        if let (Some(max), Some(actual)) = (rule.max_response_bytes, request.response_bytes) {
            if actual > max {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: response {actual} bytes exceeds max_response_bytes {max}"
                )));
            }
        }

        // SC-5 timeout budget.
        if let (Some(max), Some(actual)) = (rule.timeout_ms, request.timeout_ms) {
            if actual > max {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: timeout {actual}ms exceeds rule timeout_ms {max}"
                )));
            }
        }

        // SC-5 content-type constraints (only enforced when the rule lists any).
        if !rule.request_content_types.is_empty() {
            content_type_allowed(
                "request",
                request.request_content_type.as_deref(),
                &rule.request_content_types,
            )?;
        }
        if !rule.response_content_types.is_empty() {
            content_type_allowed(
                "response",
                request.response_content_type.as_deref(),
                &rule.response_content_types,
            )?;
        }

        // Secret-ref headers: only permitted if the rule lists the header name in
        // allow_secret_headers AND (host already matched above, so the secret is
        // bound to the allowlisted host). prd-merged/07 secrets injectInto/netHosts.
        for (name, value) in &request.headers {
            if let HeaderValue::Secret { .. } = value {
                let allowed = rule
                    .allow_secret_headers
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case(name));
                if !allowed {
                    return Err(CoreError::PermissionDenied(format!(
                        "net.fetch denied: secret header {name:?} is not in this rule's allow_secret_headers for host {:?}",
                        pattern.host
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Convenience: build a policy from a [`NetGrant`] and check one request.
pub fn check_net(allowlist: &NetGrant, request: &NetRequest) -> Result<()> {
    NetPolicy::new(allowlist).check(request)
}

/// Deny a host that is a private/loopback/metadata literal or `localhost`.
/// SC-5 `denyPrivateNetwork` default-true: refused *before* the allow rules.
fn deny_private_target(host: &str) -> Result<()> {
    if host_is_private_literal(host) {
        return Err(CoreError::PermissionDenied(format!(
            "net.fetch denied: target host {host:?} is a private/loopback/metadata address (SC-5 deny private network by default)"
        )));
    }
    Ok(())
}

/// Whether the rule's scheme rejects the request's scheme.
///
/// - an `https` rule requires an `https` request (no http downgrade);
/// - an `http` rule (explicitly chosen) accepts only `http`.
///
/// Schemes are compared case-insensitively.
fn rule_scheme_denies(rule_scheme: &str, request_scheme: &str) -> bool {
    !rule_scheme.eq_ignore_ascii_case(request_scheme)
}

/// Trailing-`*` glob match for URL paths, mirroring the storage prefix matcher:
/// `/public/*` matches any path under `/public/`; an exact pattern matches only
/// that path.
fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        path.starts_with(prefix)
    } else {
        pattern == path
    }
}

/// Enforce a content-type allowlist for the request or response. A missing
/// content-type when the rule constrains it is a denial (the host can't confirm
/// it is within policy).
fn content_type_allowed(which: &str, actual: Option<&str>, allowed: &[String]) -> Result<()> {
    match actual {
        Some(ct) if allowed.iter().any(|a| a.eq_ignore_ascii_case(ct)) => Ok(()),
        Some(ct) => Err(CoreError::PermissionDenied(format!(
            "net.fetch denied: {which} content-type {ct:?} is not in the allowlisted set {allowed:?}"
        ))),
        None => Err(CoreError::PermissionDenied(format!(
            "net.fetch denied: rule constrains {which} content-type to {allowed:?} but the request declares none"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::NetRule;

    fn grant(rules: Vec<NetRule>) -> NetGrant {
        NetGrant(rules)
    }

    fn rule(method: &str, url: &str) -> NetRule {
        NetRule { method: method.into(), url: url.into(), ..Default::default() }
    }

    fn req(method: &str, url: &str) -> NetRequest {
        NetRequest { method: method.into(), url: url.into(), ..Default::default() }
    }

    #[test]
    fn empty_allowlist_is_capability_required() {
        let g = grant(vec![]);
        let err = check_net(&g, &req("GET", "https://api.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
    }

    #[test]
    fn matching_request_is_allowed() {
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        assert!(check_net(&g, &req("GET", "https://api.example.com/public/weather")).is_ok());
    }

    #[test]
    fn host_mismatch_is_permission_denied() {
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let err = check_net(&g, &req("GET", "https://other.example.com/public/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn wildcard_host_rule_matches_nothing() {
        let g = grant(vec![rule("GET", "https://*.example.com/public/*")]);
        let err = check_net(&g, &req("GET", "https://api.example.com/public/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("wildcard host"), "{err}");
    }

    #[test]
    fn scheme_downgrade_is_denied() {
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let err = check_net(&g, &req("GET", "http://api.example.com/public/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn private_target_denied_even_if_listed() {
        // A rule literally lists localhost, but SC-5 denies it anyway.
        let g = grant(vec![rule("GET", "http://localhost/*")]);
        let err = check_net(&g, &req("GET", "http://localhost/status")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
    }

    #[test]
    fn literal_secret_header_is_denied() {
        let g = grant(vec![rule("GET", "https://api.example.com/private/*")]);
        let mut r = req("GET", "https://api.example.com/private/me");
        r.headers
            .insert("Authorization".into(), HeaderValue::Literal("Bearer x".into()));
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn secret_ref_allowed_only_on_listed_header_and_host() {
        let mut allow = rule("GET", "https://api.example.com/private/*");
        allow.allow_secret_headers = vec!["Authorization".into()];
        let g = grant(vec![allow]);
        let mut r = req("GET", "https://api.example.com/private/me");
        r.headers.insert(
            "Authorization".into(),
            HeaderValue::Secret { secret_ref: "s".into() },
        );
        assert!(check_net(&g, &r).is_ok());
    }
}
