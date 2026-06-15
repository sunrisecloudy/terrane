//! `ctx.net.fetch` host-call types and the injectable HTTP client seam.
//!
//! prd-merged/07 SC-5 (network egress policy), SC-8 (net capability grammar);
//! prd-merged/01 CR-3 (`net` namespace), **CR-8** (deterministic mode: live
//! network is forbidden unless a recorded response is being replayed).
//!
//! ## Why an injectable trait
//!
//! The runtime never depends on a real HTTP library. The *actual* HTTP is hidden
//! behind the [`HttpClient`] trait, so:
//!   * tests / CI / the demo never touch the live network — they inject a
//!     [`MockHttpClient`] returning a canned response;
//!   * the host (forge-core / a shell) provides the one concrete client that
//!     does real I/O, outside this wasm-clean crate;
//!   * in a deterministic run the response is **recorded** (record mode) and
//!     **served from the recording** (replay mode), so a replay issues no live
//!     call at all (CR-8). The client is consulted only in record mode.
//!
//! This module is **target-independent** (no QuickJS, no networking deps); it
//! compiles on `wasm32-unknown-unknown`. The request/response are plain serde
//! structs so they marshal through the `ctx.net.fetch` JS boundary and into the
//! deterministic [`RecordedCall`](forge_domain::RecordedCall) trace canonically.

use forge_domain::{CoreError, Result};
use forge_policy::HeaderValue;
pub use forge_secrets::{InMemorySecretStore, SecretStore, SecretValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A request header value the applet supplies: either a plain literal string or
/// a **secret reference** the host resolves at the HTTP edge (prd-merged/07
/// SC-13/SC-12, CR-3). The applet only ever writes the `secret_ref` *name*; it
/// never receives the resolved plaintext value (SC-13).
///
/// The JSON shape mirrors [`forge_policy::HeaderValue`] exactly so a header value
/// round-trips through the JS boundary, the policy gate, and the recorded trace:
///   * a literal is a bare string: `"Accept": "application/json"`;
///   * a secret ref is an object: `"Authorization": { "secret_ref": "name" }`.
///
/// Untagged so `serde` picks the variant by shape. The `Secret` arm is listed
/// first so an object with a `secret_ref` key is never mis-parsed as a literal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NetHeaderValue {
    /// A secret reference resolved by the host at the HTTP edge, e.g.
    /// `{ "secret_ref": "secret_weather" }`. The resolved value is **never**
    /// readable by applet JS, recorded in the trace, logged, or synced (SC-13).
    Secret {
        /// The name of the secret to resolve from the host's secret store.
        secret_ref: String,
    },
    /// A plain literal header value, e.g. `"Bearer abc"` or `"application/json"`.
    Literal(String),
}

impl NetHeaderValue {
    /// The literal string if this is a [`Literal`](Self::Literal), else `None`.
    /// A secret ref carries no readable value here (SC-13: the name is not the
    /// secret).
    pub fn as_literal(&self) -> Option<&str> {
        match self {
            NetHeaderValue::Literal(s) => Some(s.as_str()),
            NetHeaderValue::Secret { .. } => None,
        }
    }
}

/// A `ctx.net.fetch` request as the runtime hands it to an [`HttpClient`].
///
/// A plain serde struct (method/url/headers/body) so it round-trips through the
/// JS boundary and the recorded trace. `body` is an opaque string (the applet's
/// already-serialized payload); the runtime does not interpret it.
///
/// A header value is either a literal string or a `{ "secret_ref": "name" }`
/// object ([`NetHeaderValue`]). The request the applet builds (and the request
/// the recorder captures into the trace) carries the **secret_ref** — the
/// resolved secret value is injected by the host only inside the `net_fetch`
/// closure, into the request handed to the [`HttpClient`], so the trace keeps
/// only the ref and never the plaintext (SC-13 trace-safety).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetRequest {
    /// HTTP method, e.g. `GET`, `POST`.
    pub method: String,
    /// Absolute request URL (`scheme://host[:port]/path`).
    pub url: String,
    /// Request headers. Each value is a literal string or a `secret_ref` object
    /// ([`NetHeaderValue`]). A secret-bearing header is gated by the policy
    /// (`allow_secret_headers` + the destination rule) before a request ever
    /// reaches a client, and its resolved value is injected only at the HTTP edge.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, NetHeaderValue>,
    /// Optional request body, opaque to the runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional request `Content-Type` (used by the policy's content-type gate).
    /// Accepts the JS-ergonomic `contentType` alias on input; serialized as
    /// `content_type` so the recorded trace is canonical.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "contentType")]
    pub content_type: Option<String>,
    /// Optional requested timeout in milliseconds (used by the policy's timeout
    /// budget gate). Accepts the JS-ergonomic `timeoutMs` alias on input.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeoutMs")]
    pub timeout_ms: Option<u64>,
}

/// A `ctx.net.fetch` response an [`HttpClient`] returns (and the recorder
/// captures). A plain serde struct (status/headers/body) so a recorded response
/// is served byte-identically on replay.
///
/// ## Why redirect/DNS facts ride on the response (SC-5 end-to-end)
///
/// The egress [`NetPolicy`](forge_policy::NetPolicy) can re-check every redirect
/// hop and reject a DNS answer that resolves to a private address, but those
/// facts only exist **after** the client performs the fetch — they are a product
/// of the transport, not the applet's request. So the client reports them here:
///   * [`final_url`](Self::final_url) — the URL the response actually came from
///     (the last hop after any redirects), for diagnostics/diff;
///   * [`redirect_chain`](Self::redirect_chain) — the ordered hop URLs actually
///     followed (origin first, final hop last), each re-checked by the policy on
///     the response leg, so a redirect to a private IP / an unallowlisted public
///     host is denied **after** the fetch and never reaches the applet;
///   * [`dns_answers`](Self::dns_answers) — the literal addresses the host
///     resolved for the request host, so a host that resolves to a private
///     address (DNS rebinding) is denied on the response leg.
///
/// Each defaults empty and is skipped on serialize, so a response with no
/// redirects/DNS facts records byte-identically to the pre-change shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetResponse {
    /// HTTP status code, e.g. `200`.
    pub status: u16,
    /// Response headers (literal values).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Response body, opaque to the runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Optional response `Content-Type` (used by the policy's content-type gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The URL the response actually came from after any redirects (the final
    /// hop). `None` when the request was not redirected. Reported by the client
    /// so the response-leg policy check and diffs can see the real origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    /// The ordered redirect hop URLs the client actually followed (origin first,
    /// final hop last). Empty when no redirect occurred. The response-leg policy
    /// check re-runs the SC-5 request-side gates against every hop, so a
    /// redirect to a private IP or an unallowlisted public host is denied after
    /// the fetch (`redirect_to_private` / unallowlisted-hop).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_chain: Vec<String>,
    /// The literal addresses the host resolved for the request host, if the
    /// client reports them. Empty when DNS facts are not available. A private
    /// literal answer is denied on the response leg (DNS rebinding to a private
    /// address — `dns_rebinding_to_private`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_answers: Vec<String>,
}

impl NetResponse {
    /// The response body's byte length (for the SC-5 response-size budget). A
    /// missing body is zero bytes.
    pub fn body_bytes(&self) -> u64 {
        self.body.as_ref().map(|b| b.len() as u64).unwrap_or(0)
    }
}

/// The injectable HTTP seam. The runtime holds a `&dyn HttpClient` (via the
/// [`HostBridge`](crate::HostBridge)) and calls [`send`](HttpClient::send) only
/// in **record mode**; on replay the recorder serves the recorded response and
/// no client is consulted (CR-8). The one concrete, real-I/O implementor lives
/// host-side (forge-core / a shell), never in this crate, so the runtime stays
/// wasm-clean and CI never makes a live request.
pub trait HttpClient {
    /// Perform `request` and return the response, or a [`CoreError`] on a
    /// transport failure. Implementors must never panic on a failed request —
    /// they return `Err` so the run surfaces a `CoreError` outcome.
    ///
    /// A real client that follows redirects / resolves DNS must populate the
    /// response's [`final_url`](NetResponse::final_url),
    /// [`redirect_chain`](NetResponse::redirect_chain) (the ordered hop URLs it
    /// actually followed) and [`dns_answers`](NetResponse::dns_answers) (the
    /// literal addresses it resolved), so the runtime's response-leg policy check
    /// can re-run SC-5 against the real hops/answers and deny a
    /// redirect-to-private / DNS-rebind-to-private / unallowlisted-hop response
    /// *after* the fetch, before anything reaches the applet.
    fn send(&self, request: NetRequest) -> Result<NetResponse>;
}

/// A deterministic, network-free [`HttpClient`] for tests, CI, the demo, and the
/// default [`MemoryHostBridge`](crate::MemoryHostBridge): it returns a canned
/// response and never opens a socket. NO live network ever runs through this.
#[derive(Debug, Clone)]
pub struct MockHttpClient {
    response: NetResponse,
}

impl MockHttpClient {
    /// A mock that always returns `response`.
    pub fn new(response: NetResponse) -> Self {
        MockHttpClient { response }
    }

    /// The default canned response: `200` with a tiny JSON body. Enough for the
    /// runtime's own record/replay tests to assert a deterministic value.
    pub fn canned() -> Self {
        MockHttpClient {
            response: NetResponse {
                status: 200,
                body: Some(r#"{"ok":true}"#.to_string()),
                content_type: Some("application/json".to_string()),
                ..Default::default()
            },
        }
    }

    /// A mock that simulates a redirect chain: it returns a `200` JSON response
    /// whose `redirect_chain`/`final_url` report `hops` (origin first, final hop
    /// last), as a real client following redirects would. No live network: this
    /// lets the runtime's response-leg policy check see the hops a fetch followed
    /// so a redirect to a private/unallowlisted host is denied after the fetch.
    pub fn with_redirects(hops: Vec<String>) -> Self {
        let final_url = hops.last().cloned();
        MockHttpClient {
            response: NetResponse {
                status: 200,
                body: Some(r#"{"ok":true}"#.to_string()),
                content_type: Some("application/json".to_string()),
                final_url,
                redirect_chain: hops,
                ..Default::default()
            },
        }
    }

    /// A mock that simulates the host resolving to `answers` (literal addresses),
    /// as a DNS-pinning client would report. No live network: this lets the
    /// response-leg policy check catch a host that resolves to a private address
    /// (DNS rebinding) after the fetch.
    pub fn with_dns_answers(answers: Vec<String>) -> Self {
        MockHttpClient {
            response: NetResponse {
                status: 200,
                body: Some(r#"{"ok":true}"#.to_string()),
                content_type: Some("application/json".to_string()),
                dns_answers: answers,
                ..Default::default()
            },
        }
    }
}

impl Default for MockHttpClient {
    fn default() -> Self {
        MockHttpClient::canned()
    }
}

impl HttpClient for MockHttpClient {
    fn send(&self, _request: NetRequest) -> Result<NetResponse> {
        Ok(self.response.clone())
    }
}

/// Resolve every `secret_ref` header in `request` to its plaintext value using
/// `store`, returning a **new** request whose headers are all literal — ready to
/// hand to the [`HttpClient`]. The input `request` (which the recorder captured)
/// is left untouched, so the trace keeps the `secret_ref` and never the resolved
/// value (SC-13 trace-safety).
///
/// This runs **after** the policy has already gated each secret header against
/// the destination rule's `allow_secret_headers` (so reaching here means the
/// header/destination pair is allowlisted). An unknown/revoked name is a flat
/// `RuntimeError` and the request is never sent — fail-closed.
///
/// Literal headers pass through unchanged. The function performs no networking
/// and no logging; the resolved value exists only on the returned request, which
/// the caller hands straight to the client and then drops.
pub fn resolve_secret_headers(
    request: &NetRequest,
    store: &dyn SecretStore,
) -> Result<NetRequest> {
    let allow_secret_headers = request
        .headers
        .iter()
        .filter(|(_, value)| matches!(value, NetHeaderValue::Secret { .. }))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    resolve_secret_headers_with_allowlist(request, &allow_secret_headers, store)
}

/// Resolve `secret_ref` headers using the canonical [`forge_secrets`] injector
/// and the destination rule's already-policy-checked `allow_secret_headers`.
///
/// Runtime owns the `NetRequest`/`NetHeaderValue` wire shape, while
/// `forge-secrets` owns the redacting store/value API and host-edge injection
/// semantics. This adapter converts between the two wire-equivalent header
/// enums and returns a fresh literal-only [`NetRequest`] for the HTTP client.
pub fn resolve_secret_headers_with_allowlist(
    request: &NetRequest,
    allow_secret_headers: &[String],
    store: &dyn SecretStore,
) -> Result<NetRequest> {
    let policy_headers = request
        .headers
        .iter()
        .map(|(name, value)| {
            let value = match value {
                NetHeaderValue::Secret { secret_ref } => HeaderValue::Secret {
                    secret_ref: secret_ref.clone(),
                },
                NetHeaderValue::Literal(value) => HeaderValue::Literal(value.clone()),
            };
            (name.clone(), value)
        })
        .collect();
    let resolved_headers =
        forge_secrets::resolve_secret_headers(&policy_headers, allow_secret_headers, store)?;
    let mut resolved = request.clone();
    resolved.headers = resolved_headers
        .into_iter()
        .map(|(name, value)| (name, NetHeaderValue::Literal(value)))
        .collect();
    Ok(resolved)
}

/// Build the canonical "live network forbidden" error for a deterministic run
/// whose client refuses to perform a live request (CR-8). Used by bridges that
/// have no client wired (e.g. the replay [`NullBridge`](crate::NullBridge)).
pub fn live_network_forbidden(method: &str) -> CoreError {
    CoreError::RuntimeError(format!(
        "ctx.net.fetch ({method}) attempted a live network call in a context with no HTTP client; \
         live network is forbidden unless a recorded response is being replayed (CR-8)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_client_returns_canned_response() {
        let c = MockHttpClient::canned();
        let resp = c
            .send(NetRequest {
                method: "GET".into(),
                url: "https://api.example.com/x".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_deref(), Some(r#"{"ok":true}"#));
    }

    #[test]
    fn net_request_response_roundtrip_canonical_json() {
        let req = NetRequest {
            method: "POST".into(),
            url: "https://api.example.com/forms/submit".into(),
            body: Some("{}".into()),
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let back: NetRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(req, back);

        let resp = NetResponse {
            status: 201,
            body: Some("created".into()),
            ..Default::default()
        };
        let back: NetResponse =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(resp, back);
        assert_eq!(resp.body_bytes(), 7);
    }

    #[test]
    fn empty_redirect_dns_fields_are_skipped_so_recording_is_byte_identical() {
        // A response with no redirect/DNS facts must serialize EXACTLY as before
        // the SC-5 facts were added (no `final_url`/`redirect_chain`/`dns_answers`
        // keys), so the allowed-case recording stays byte-identical.
        let resp = NetResponse {
            status: 200,
            body: Some(r#"{"ok":true}"#.into()),
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"status":200,"body":"{\"ok\":true}","content_type":"application/json"}"#);
    }

    #[test]
    fn redirect_and_dns_fields_roundtrip_when_present() {
        let resp = NetResponse {
            status: 200,
            body: Some(r#"{"ok":true}"#.into()),
            content_type: Some("application/json".into()),
            final_url: Some("https://cdn.example.com/x".into()),
            redirect_chain: vec![
                "https://api.example.com/x".into(),
                "https://cdn.example.com/x".into(),
            ],
            dns_answers: vec!["203.0.113.7".into()],
            ..Default::default()
        };
        let back: NetResponse =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn with_redirects_mock_reports_chain_and_final_url() {
        let c = MockHttpClient::with_redirects(vec![
            "https://api.example.com/x".into(),
            "https://cdn.example.com/x".into(),
        ]);
        let resp = c
            .send(NetRequest {
                method: "GET".into(),
                url: "https://api.example.com/x".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(resp.redirect_chain.len(), 2);
        assert_eq!(resp.final_url.as_deref(), Some("https://cdn.example.com/x"));
    }

    #[test]
    fn with_dns_answers_mock_reports_answers() {
        let c = MockHttpClient::with_dns_answers(vec!["127.0.0.1".into()]);
        let resp = c
            .send(NetRequest {
                method: "GET".into(),
                url: "https://api.example.com/x".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(resp.dns_answers, vec!["127.0.0.1".to_string()]);
    }

    #[test]
    fn live_network_forbidden_is_runtime_error() {
        assert_eq!(live_network_forbidden("GET").code(), "RuntimeError");
    }

    // --- SC-13: SecretStore + secret-header resolver -------------------------

    #[test]
    fn in_memory_secret_store_resolves_known_and_misses_unknown() {
        let store = InMemorySecretStore::from_pairs([("a", "alpha"), ("b", "beta")]);
        assert_eq!(store.get("a").unwrap().unwrap().expose(), "alpha");
        assert_eq!(store.get("b").unwrap().unwrap().expose(), "beta");
        assert!(store.get("missing").unwrap().is_none());
        // The empty default resolves nothing (fail-closed).
        assert!(InMemorySecretStore::new().get("a").unwrap().is_none());
    }

    #[test]
    fn in_memory_secret_store_debug_redacts_values() {
        // SC-13: a `{:?}` on the store must print names but NEVER the values.
        let store = InMemorySecretStore::new().with_secret("api_key", "super-secret-value");
        let dbg = format!("{store:?}");
        assert!(dbg.contains("api_key"), "names are visible: {dbg}");
        assert!(!dbg.contains("super-secret-value"), "values must be redacted: {dbg}");
        assert!(dbg.contains("<redacted>"), "redaction marker present: {dbg}");
    }

    #[test]
    fn resolve_secret_headers_injects_value_and_leaves_input_untouched() {
        let store = InMemorySecretStore::new().with_secret("tok", "Bearer XYZ");
        let mut request = NetRequest {
            method: "GET".into(),
            url: "https://api.example.com/x".into(),
            ..Default::default()
        };
        request
            .headers
            .insert("Authorization".into(), NetHeaderValue::Secret { secret_ref: "tok".into() });
        request
            .headers
            .insert("Accept".into(), NetHeaderValue::Literal("application/json".into()));

        let resolved = resolve_secret_headers(&request, &store).unwrap();
        // The RESOLVED request carries the literal value; the literal header is
        // unchanged.
        assert_eq!(
            resolved.headers.get("Authorization").and_then(|h| h.as_literal()),
            Some("Bearer XYZ")
        );
        assert_eq!(
            resolved.headers.get("Accept").and_then(|h| h.as_literal()),
            Some("application/json")
        );
        // The INPUT request still holds the secret_ref (the recorder keeps this).
        assert!(matches!(
            request.headers.get("Authorization"),
            Some(NetHeaderValue::Secret { .. })
        ));
    }

    #[test]
    fn resolve_secret_headers_errors_on_unknown_secret() {
        let store = InMemorySecretStore::new(); // empty
        let mut request = NetRequest {
            method: "GET".into(),
            url: "https://api.example.com/x".into(),
            ..Default::default()
        };
        request
            .headers
            .insert("Authorization".into(), NetHeaderValue::Secret { secret_ref: "nope".into() });
        let err = resolve_secret_headers(&request, &store).unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        assert!(err.to_string().contains("nope"), "error names the ref: {err}");
        // The error message must not contain any resolved value (there is none).
    }

    #[test]
    fn resolve_secret_headers_rechecks_allowlist() {
        let store = InMemorySecretStore::new().with_secret("tok", "Bearer XYZ");
        let mut request = NetRequest {
            method: "GET".into(),
            url: "https://api.example.com/x".into(),
            ..Default::default()
        };
        request
            .headers
            .insert("Authorization".into(), NetHeaderValue::Secret { secret_ref: "tok".into() });

        let err = resolve_secret_headers_with_allowlist(&request, &[], &store).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("Authorization"));
        assert!(!err.to_string().contains("Bearer XYZ"));
    }

    #[test]
    fn net_header_value_serde_picks_secret_vs_literal_by_shape() {
        // A bare string → Literal; an object with `secret_ref` → Secret.
        let lit: NetHeaderValue = serde_json::from_str(r#""application/json""#).unwrap();
        assert_eq!(lit, NetHeaderValue::Literal("application/json".into()));
        let sec: NetHeaderValue = serde_json::from_str(r#"{"secret_ref":"k"}"#).unwrap();
        assert_eq!(sec, NetHeaderValue::Secret { secret_ref: "k".into() });
        // Round-trips canonically.
        assert_eq!(serde_json::to_string(&sec).unwrap(), r#"{"secret_ref":"k"}"#);
        assert_eq!(serde_json::to_string(&lit).unwrap(), r#""application/json""#);
        // The secret_ref name is NOT readable as a literal value.
        assert_eq!(sec.as_literal(), None);
    }
}
