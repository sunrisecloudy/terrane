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
//!   - every redirect hop is re-checked the same way as the original request
//!     (SC-5 "redirect targets are checked the same way"): each hop URL must
//!     pass the private-network deny **and** independently satisfy an allow
//!     rule's request-side constraints (scheme/host/path/method + header/secret),
//!     so a hop to a public-but-unallowlisted origin/path is denied. A redirect
//!     chain whose hops are *literal* private IPs is caught here; a redirect to a
//!     private *hostname* that only resolves privately is a runtime concern;
//!   - `dns_answers` carrying a private literal IP **is** caught here as a
//!     best-effort literal check, but the authoritative DNS-pin/recheck is a
//!     runtime concern.
//!
//! The T011 fixtures flag the vectors that need the runtime resolver via
//! `requires_runtime_dns`; the data-driven test asserts those separately.

use crate::net_url::{host_is_private_literal, ParsedUrl};
use forge_domain::{is_supported_net_scheme, CoreError, NetGrant, NetRule, Result};
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

fn is_simple_literal_header_name(name: &str) -> bool {
    const SIMPLE_HEADERS: &[&str] = &[
        "accept",
        "accept-language",
        "content-language",
        "content-type",
    ];
    let lower = name.to_ascii_lowercase();
    SIMPLE_HEADERS.contains(&lower.as_str())
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
    ///     (rejected), a disallowed secret header, or a **redirect hop** that is
    ///     not itself covered by an allow rule.
    ///
    /// Order: the **private-network deny** runs first on the request host and
    /// every redirect hop / DNS answer, so a private target is refused even if a
    /// rule literally names it. Then the original request is matched against the
    /// full rule set (request side plus response-side caps). Finally, SC-5
    /// requires redirects to be re-checked: **every** redirect hop URL must
    /// independently satisfy an allow rule's request-side constraints
    /// (scheme/host/path/method, plus header/secret constraints), so a hop to a
    /// public-but-unallowlisted origin or path is denied even though the original
    /// request was allowed.
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
        if !is_supported_net_scheme(&target.scheme) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: unsupported url scheme {:?}; only http and https are allowed",
                target.scheme
            )));
        }

        // SC-5 private-network deny: the request host, every redirect hop, and
        // every literal DNS answer must not be a private/loopback/metadata
        // literal or the `localhost` hostname — denied before any allow rule.
        deny_private_target(&target.host)?;
        let hops = self.parse_redirect_hops(request)?;
        for hop_url in &hops {
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

        validate_literal_headers(request)?;

        // The original request must match a rule on the full constraint set
        // (request side + response-side size/content-type caps). Find the first
        // rule that grants it; if none does, surface why (the capability IS
        // declared, this request just isn't covered).
        self.match_full_request(request, &target)?;

        // SC-5: redirects are re-checked the same way. Every redirect hop URL
        // must independently satisfy an allow rule's request-side constraints
        // (scheme/host/path/method + header/secret constraints), not merely the
        // private-IP check above. A hop to a public-but-unallowlisted origin or
        // path is denied even though the original request was allowed. Response-
        // side caps (max_response_bytes / response_content_types) are evaluated
        // once against the final response on the original request, so the per-hop
        // re-check only covers the request side. (docs/24: "redirects to a
        // disallowed origin are rejected".)
        for (idx, hop_url) in hops.iter().enumerate() {
            self.match_request_side(request, hop_url).map_err(|reason| {
                CoreError::PermissionDenied(format!(
                    "net.fetch denied: redirect hop {} {:?} is not covered by any net rule (redirects are re-checked, SC-5): {reason}",
                    idx,
                    request.redirect_chain[idx],
                ))
            })?;
        }

        Ok(())
    }

    /// Return the matched rule's `allow_secret_headers` after running the same
    /// SC-5 decision as [`check`](Self::check). The runtime feeds this to the
    /// host-edge secret injector's defense-in-depth allowlist.
    pub fn allowed_secret_headers(&self, request: &NetRequest) -> Result<Vec<String>> {
        self.check(request)?;
        let target = ParsedUrl::parse(&request.url).map_err(|e| {
            CoreError::PermissionDenied(format!(
                "net.fetch denied: invalid url {:?}: {e}",
                request.url
            ))
        })?;
        let rule = self.match_full_rule(request, &target)?;
        Ok(rule.allow_secret_headers.clone())
    }

    /// Parse every redirect-chain hop into a [`ParsedUrl`], failing closed on a
    /// malformed hop URL. Order is preserved (origin first, final hop last).
    fn parse_redirect_hops(&self, request: &NetRequest) -> Result<Vec<ParsedUrl>> {
        request
            .redirect_chain
            .iter()
            .map(|hop| {
                ParsedUrl::parse(hop).map_err(|e| {
                    CoreError::PermissionDenied(format!(
                        "net.fetch denied: invalid redirect target {hop:?}: {e}"
                    ))
                })
            })
            .collect()
    }

    /// Run the **full** rule match (request side + response-side caps) for the
    /// original request against `target`. `Ok` if some rule grants it; otherwise
    /// the first rule's denial reason (or a generic "no rule covers" message).
    fn match_full_request(&self, request: &NetRequest, target: &ParsedUrl) -> Result<()> {
        self.match_full_rule(request, target).map(|_| ())
    }

    /// Return the first rule that fully grants the original request.
    fn match_full_rule<'rule>(
        &'rule self,
        request: &NetRequest,
        target: &ParsedUrl,
    ) -> Result<&'rule NetRule> {
        let mut last_reason: Option<CoreError> = None;
        for rule in self.allowlist.rules() {
            match self.rule_matches(rule, request, target) {
                Ok(()) => return Ok(rule),
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

    /// Run only the **request-side** constraints (scheme/host/path/method +
    /// header/secret constraints) for `target` against the allowlist, ignoring
    /// response-side caps. Used to re-check each redirect hop. `Ok` if some rule
    /// grants the hop; otherwise the first rule's denial reason.
    fn match_request_side(&self, request: &NetRequest, target: &ParsedUrl) -> Result<()> {
        let mut last_reason: Option<CoreError> = None;
        for rule in self.allowlist.rules() {
            match self.rule_matches_request_side(rule, request, target) {
                Ok(()) => return Ok(()),
                Err(reason) => last_reason = Some(reason),
            }
        }
        Err(last_reason.unwrap_or_else(|| {
            CoreError::PermissionDenied(format!(
                "net.fetch denied: no net rule covers host {:?} path {:?}",
                target.host, target.path
            ))
        }))
    }

    /// Check a single rule against the original request. `Ok` if this rule fully
    /// grants the request (request side **and** response-side caps); `Err` names
    /// the first constraint this rule fails (used as the denial reason when no
    /// rule matches).
    fn rule_matches(&self, rule: &NetRule, request: &NetRequest, target: &ParsedUrl) -> Result<()> {
        // Request side first (scheme/host/path/method + body/timeout/request
        // content-type + secret headers). This is the subset re-checked per hop.
        self.rule_matches_request_side(rule, request, target)?;

        // Response-side caps only apply to the final response of the original
        // request, so they are NOT part of the per-hop re-check.
        if let (Some(max), Some(actual)) = (rule.max_response_bytes, request.response_bytes) {
            if actual > max {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: response {actual} bytes exceeds max_response_bytes {max}"
                )));
            }
        }
        if !rule.response_content_types.is_empty() {
            content_type_allowed(
                "response",
                request.response_content_type.as_deref(),
                &rule.response_content_types,
            )?;
        }

        Ok(())
    }

    /// Check a single rule's **request-side** constraints against `target`:
    /// scheme/host/path/method, request body size, timeout budget, request
    /// content-type, and secret-bearing headers. This is exactly the subset that
    /// SC-5 requires re-running against every redirect hop, so it is factored out
    /// from the response-side caps. `Ok` if this rule grants the request side;
    /// `Err` names the first failing constraint.
    fn rule_matches_request_side(
        &self,
        rule: &NetRule,
        request: &NetRequest,
        target: &ParsedUrl,
    ) -> Result<()> {
        let pattern = ParsedUrl::parse(&rule.url).map_err(|e| {
            CoreError::PermissionDenied(format!(
                "net.fetch denied: malformed net rule url {:?}: {e}",
                rule.url
            ))
        })?;
        if !is_supported_net_scheme(&pattern.scheme) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: unsupported net rule scheme {:?}; only http and https are allowed",
                pattern.scheme
            )));
        }
        if !is_supported_net_scheme(&target.scheme) {
            return Err(CoreError::PermissionDenied(format!(
                "net.fetch denied: unsupported url scheme {:?}; only http and https are allowed",
                target.scheme
            )));
        }

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

        // SC-5 request body size cap.
        if let (Some(max), Some(actual)) = (rule.max_body_bytes, request.body_bytes) {
            if actual > max {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: request body {actual} bytes exceeds max_body_bytes {max}"
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

        // SC-5 request content-type constraint (only enforced when the rule
        // lists any).
        if !rule.request_content_types.is_empty() {
            content_type_allowed(
                "request",
                request.request_content_type.as_deref(),
                &rule.request_content_types,
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

fn validate_literal_headers(request: &NetRequest) -> Result<()> {
    for (name, value) in &request.headers {
        if let HeaderValue::Literal(_) = value {
            if is_secret_header_name(name) {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: secret-like header {name:?} carries a literal value; route secrets through the secret-injection policy (allow_secret_headers), not a literal"
                )));
            }
            if !is_simple_literal_header_name(name) {
                return Err(CoreError::PermissionDenied(format!(
                    "net.fetch denied: literal header {name:?} is not allowed by the net policy; only simple literal headers are permitted without an explicit manifest constraint"
                )));
            }
        }
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
    fn literal_non_simple_header_is_denied() {
        let g = grant(vec![rule("GET", "https://api.example.com/private/*")]);
        let mut r = req("GET", "https://api.example.com/private/me");
        r.headers
            .insert("X-Api-Key".into(), HeaderValue::Literal("literal-key".into()));
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("literal header"), "{err}");

        let mut simple = req("GET", "https://api.example.com/private/me");
        simple
            .headers
            .insert("Accept".into(), HeaderValue::Literal("application/json".into()));
        assert!(check_net(&g, &simple).is_ok());
    }

    #[test]
    fn unsupported_scheme_rule_and_request_are_denied() {
        let g = grant(vec![rule("GET", "ftp://api.example.com/public/*")]);
        let err = check_net(&g, &req("GET", "ftp://api.example.com/public/file")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("unsupported"), "{err}");
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

    #[test]
    fn allowed_secret_headers_returns_the_matching_rule_only() {
        let mut first = rule("GET", "https://api.example.com/private/*");
        first.allow_secret_headers = vec!["Authorization".into()];
        let mut second = rule("GET", "https://cdn.example.com/public/*");
        second.allow_secret_headers = vec!["X-Api-Key".into()];
        let g = grant(vec![first, second]);

        let headers = NetPolicy::new(&g)
            .allowed_secret_headers(&req("GET", "https://cdn.example.com/public/asset"))
            .unwrap();
        assert_eq!(headers, vec!["X-Api-Key".to_string()]);
    }

    // --- SC-5 redirect re-check (review 069 P1) ------------------------------

    #[test]
    fn public_redirect_every_hop_allowlisted_is_allowed() {
        // T011 public_redirect_to_public_allowed: both hops are allowlisted.
        let g = grant(vec![
            rule("GET", "https://api.example.com/public/*"),
            rule("GET", "https://cdn.example.com/public/*"),
        ]);
        let mut r = req("GET", "https://api.example.com/public/asset");
        r.redirect_chain = vec![
            "https://api.example.com/public/asset".into(),
            "https://cdn.example.com/public/asset".into(),
        ];
        assert!(check_net(&g, &r).is_ok());
    }

    #[test]
    fn redirect_hop_to_private_is_denied() {
        // T011 redirect_to_private_denied: final hop is a loopback literal.
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let mut r = req("GET", "https://api.example.com/public/redirect");
        r.redirect_chain = vec![
            "https://api.example.com/public/redirect".into(),
            "http://127.0.0.1/admin".into(),
        ];
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
    }

    #[test]
    fn redirect_hop_to_public_but_unallowlisted_origin_is_denied() {
        // The decisive new case: the redirect target is public (not private) but
        // its origin is NOT in the allowlist. Pre-fix this passed because only the
        // original URL was rule-checked; SC-5 requires every hop re-checked.
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let mut r = req("GET", "https://api.example.com/public/asset");
        r.redirect_chain = vec![
            "https://api.example.com/public/asset".into(),
            "https://evil.example.net/public/asset".into(),
        ];
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("redirect hop"), "{err}");
    }

    #[test]
    fn redirect_hop_to_unallowlisted_path_on_allowlisted_host_is_denied() {
        // Same allowlisted host, but the hop path escapes the allowed glob.
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let mut r = req("GET", "https://api.example.com/public/asset");
        r.redirect_chain = vec![
            "https://api.example.com/public/asset".into(),
            "https://api.example.com/admin/secret".into(),
        ];
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("redirect hop"), "{err}");
    }

    #[test]
    fn malformed_redirect_hop_fails_closed() {
        let g = grant(vec![rule("GET", "https://api.example.com/public/*")]);
        let mut r = req("GET", "https://api.example.com/public/asset");
        r.redirect_chain = vec!["not-a-url".into()];
        let err = check_net(&g, &r).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }
}
