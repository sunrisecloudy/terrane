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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A `ctx.net.fetch` request as the runtime hands it to an [`HttpClient`].
///
/// A plain serde struct (method/url/headers/body) so it round-trips through the
/// JS boundary and the recorded trace. `body` is an opaque string (the applet's
/// already-serialized payload); the runtime does not interpret it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetRequest {
    /// HTTP method, e.g. `GET`, `POST`.
    pub method: String,
    /// Absolute request URL (`scheme://host[:port]/path`).
    pub url: String,
    /// Request headers (literal values). Secret-bearing headers are gated by the
    /// policy before a request ever reaches a client.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
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
                headers: BTreeMap::new(),
                body: Some(r#"{"ok":true}"#.to_string()),
                content_type: Some("application/json".to_string()),
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
    fn live_network_forbidden_is_runtime_error() {
        assert_eq!(live_network_forbidden("GET").code(), "RuntimeError");
    }
}
