//! `ctx.net.fetch` for [`HostContext`]: the SC-5 egress-policy-checked, recorded
//! network handler plus its request/response policy projections and the SC-13
//! trace-safety helpers.
//!
//! The handler keeps every effect inside `recorder.host_call(method, args, ||
//! bridge_call)` and resolves secret-ref headers (SC-13) **inside** that closure,
//! at the HTTP edge, so the recorded trace keeps only the `secret_ref` while the
//! client receives the resolved value. The response leg re-checks the same
//! [`NetPolicy`] against the populated response (size/content-type +
//! redirect/DNS facts) and redacts a denied response out of the trace.

use super::HostContext;
use crate::net::{
    resolve_secret_headers_with_allowlist, NetHeaderValue, NetRequest, NetResponse,
};
use crate::recorder::recorded_denial_error;
use forge_domain::{CoreError, NetGrant, Result};
use forge_policy::NetPolicy;

impl HostContext<'_> {
    // --- Net (egress-policy-checked, recorded) --------------------------

    /// `ctx.net.fetch(request)` — perform a network request, gated by the SC-5
    /// network egress policy and recorded for deterministic replay.
    ///
    /// Order (prd-merged/07 SC-5, prd-merged/01 CR-3/CR-4/CR-8):
    ///   1. **Role gate** (SC-10): a non-runnable actor cannot fetch — denied
    ///      before any policy/bridge work, recorded as the run's denial.
    ///   2. **Egress policy** ([`NetPolicy`], capability check at call time, CR-4):
    ///      the request is matched against the manifest's `net` allowlist. An
    ///      empty allowlist ⇒ `CapabilityRequired`; a non-matching/forbidden
    ///      request (host/scheme/path/method mismatch, a private-IP/localhost
    ///      target denied by default, a size/timeout/content-type violation, a
    ///      secret-header violation) ⇒ `PermissionDenied`. A denied fetch is
    ///      recorded as the run's denial and **no request reaches the client**.
    ///   3. **Host-call budget** (SC-2): a permitted fetch counts against
    ///      `max_host_calls` (the `NetPolicy` decision is separate from the
    ///      `PolicyEngine` category counter, so net counts its own calls here,
    ///      like `ctx.log`).
    ///   4. **Record/replay** (CR-8): in record mode the response the bridge's
    ///      injected [`HttpClient`](crate::HttpClient) returned is captured; on
    ///      replay the recorded response is **served** and no live call is made —
    ///      live network is forbidden unless a recorded response is being served.
    pub fn net_fetch(&mut self, request: NetRequest) -> Result<NetResponse> {
        // The args RECORDED into the trace are trace-safe (SC-13): a `secret_ref`
        // header is kept verbatim (it carries only the non-sensitive name), but a
        // *literal* value on a secret-like header (Authorization/Cookie/…) is
        // redacted to a placeholder, so even a request the applet wrote with a
        // plaintext secret as a literal — which the policy denies — cannot persist
        // that plaintext in the RunRecord. The request handed to the bridge below
        // is the ORIGINAL (unredacted) one; only the recorded copy is redacted.
        let args = trace_safe_args(&request);

        // 1. Role gate (SC-10): record the denial so it is replayable, then fail.
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for net.fetch call".to_string(),
            );
            self.recorder.record_denial("net.fetch", args, &err)?;
            return Err(err);
        }

        // 1b. Secret-exfil guard (SC-13 / spec/secrets.md): M0a permits a
        //     `secret_ref` ONLY in a header value. A secret_ref smuggled into the
        //     request BODY is a secret-exfil pattern — reject it as a
        //     ValidationError before any policy/budget/bridge work, and never send.
        //     This is recorded as the run's denial so it is replayable. (The body
        //     is opaque to the runtime, so this is a textual scan for the
        //     `secret_ref` marker; a literal body that happens to contain the
        //     marker is treated conservatively as exfil — fail-closed.)
        if let Some(body) = &request.body {
            if body_contains_secret_ref(body) {
                let err = CoreError::ValidationError(
                    "ctx.net.fetch denied: a secret_ref may only appear in a header value, not the request body (SC-13 secret-exfil guard)".to_string(),
                );
                self.recorder.record_denial("net.fetch", args, &err)?;
                return Err(err);
            }
        }

        // 2. Egress call gate (SC-5 / CR-4): request-side gates only, decided
        //    BEFORE the request is sent so no request reaches the client on a
        //    deny. Evaluated against the request-phase allowlist (response
        //    constraints stripped) so a rule that caps the *response* does not
        //    spuriously deny here, where the response is still unknown — those
        //    caps are enforced, intact, on the response leg (step 5). A denial is
        //    recorded then propagated.
        let policy_request = to_policy_request(&request);
        let allow_secret_headers = match NetPolicy::new(&self.net_allowlist_request_phase)
            .allowed_secret_headers(&policy_request)
        {
            Ok(headers) => headers,
            Err(net_err) => {
                self.recorder.record_denial("net.fetch", args, &net_err)?;
                return Err(net_err);
            }
        };

        // 3. Host-call budget (SC-2): only a permitted fetch consumes a slot.
        self.budgets.check_net_call()?;

        // 4. Record/replay (CR-8) + secret injection (SC-13). The `args` recorded
        //    into the trace are the request AS THE APPLET BUILT IT — every
        //    secret-bearing header still carries only its `{ secret_ref }`, never a
        //    resolved value. INSIDE the closure (record mode only) the host
        //    resolves each secret_ref against the bridge's secret store and hands
        //    the RESOLVED, literal-only request to the client. So:
        //      * the client/bridge sees the real header value;
        //      * the RECORDING (`args`) keeps only the secret_ref;
        //      * replay serves the recorded response and never resolves a secret
        //        (the closure does not run), so replay needs no secret store.
        //    The secret-header/destination allowlist gate already ran at step 2
        //    (the policy permits a secret_ref header only where the matched rule
        //    lists it in `allow_secret_headers`); an unknown/revoked secret name is
        //    a fail-closed `RuntimeError` here and no request is sent.
        let bridge = &mut *self.bridge;
        let req = request.clone();
        let response_json = self.recorder.host_call("net.fetch", args, || {
            // Resolve secret_ref headers to plaintext only now, at the HTTP edge,
            // into a fresh literal-only request the client receives. `req` (the
            // recorded shape) is untouched, so the trace keeps the secret_ref.
            let injected = resolve_secret_headers_with_allowlist(
                &req,
                &allow_secret_headers,
                bridge.secret_store(),
            )?;
            let resp = bridge.net_fetch(injected)?;
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("net.fetch response serialize failed: {e}"))
            })
        })?;
        // On REPLAY the recorder serves the recorded response. A denied fetch was
        // REDACTED into a denial-shaped entry (`record_denial` for a request-gate
        // deny, `redact_last_response` for the response-leg deny in step 5 below),
        // so the recorded entry for a denied fetch is that shape — NOT a
        // `NetResponse`. Reconstruct the original denial here and surface it, so
        // replay reports the SAME error byte-identically instead of failing to
        // decode the redacted entry as a `NetResponse` (review 077). A real recorded
        // response is a full `NetResponse` (always carries `status`); current
        // redactions carry the reserved `__forge_denial: true` marker, and legacy
        // redactions used the same `denied`/`secret_injected` shape without that
        // marker. Both are control metadata, not user response bytes, for net.fetch.
        if let Some(err) = recorded_denial_error(&response_json) {
            return Err(err);
        }
        let response = serde_json::from_value::<NetResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("net.fetch response decode failed: {e}"))
        })?;

        // 5. Response-leg egress check (SC-5 response caps + redirect/DNS facts):
        //    the call-gate check above could only see the *request* — the response
        //    size/content-type, the redirect hops actually followed, and the
        //    resolved DNS answers do not exist until the fetch returns. Re-run the
        //    SAME `NetPolicy` against the populated response (size/content-type +
        //    `redirect_chain` + `dns_answers` reported by the client) before the
        //    body is served to the applet, so:
        //      * an over-cap / wrong-content-type response is denied;
        //      * a redirect to a private IP or an unallowlisted public host is
        //        denied (every hop is re-checked against the request-side gates);
        //      * a host that resolves to a private literal address (DNS rebinding)
        //        is denied.
        //    This re-check runs on **both** record and replay (the recorded
        //    response is policy-bound too: a tampered/oversized/rebinding recording
        //    is denied identically on replay), and is fail-closed — a violating
        //    response surfaces as the run's `CoreError` and never reaches the applet.
        //
        //    TRACE-SAFETY (review 074 #2 / SC-13): on a response-leg DENIAL the
        //    response captured by `host_call` is REDACTED into a denial-shaped
        //    trace entry, so a rejected response body — and any value that a
        //    secret-bearing request might otherwise expose downstream — never
        //    persists in the RunRecord. The call's recorded `args` still carry only
        //    the request's `secret_ref` (never a resolved value).
        let response_policy_request = to_response_policy_request(&request, &response);
        if let Err(net_err) = NetPolicy::new(&self.net_allowlist).check(&response_policy_request) {
            // Review 153: by here the request's secret_ref headers were already
            // RESOLVED and SENT over the wire (step 4 resolved them inside the
            // closure, before this response-leg check). A secret that crossed the
            // trust boundary must still be auditable, so mark the redaction with the
            // non-sensitive fact that injection happened — the audit builder emits a
            // `secret.use` row for it even though the call is denied here. The marker
            // carries no secret value; it only records THAT a secret_ref header was
            // present (and therefore injected) on this request.
            let secret_injected = request_carries_secret_ref(&request);
            self.recorder.redact_last_response(&net_err, secret_injected);
            return Err(net_err);
        }

        Ok(response)
    }
}

/// Project the runtime's [`NetRequest`] onto the [`forge_policy::NetRequest`] the
/// egress [`NetPolicy`] evaluates **at the call gate**. The runtime carries the
/// *wire* request (method/url/headers/body/content-type/timeout); the policy
/// needs the match-relevant fields plus the declared body size for the SC-5 size
/// cap. The response-size/content-type caps cannot be evaluated here (the
/// response isn't known yet at the call gate); they are enforced on the response
/// leg by [`to_response_policy_request`] + a second [`NetPolicy`] check after the
/// bridge returns (`net_fetch` step 5). The literal URL/host/scheme/path/method/
/// body-size/timeout/secret-header gates are what this call-time check enforces.
fn to_policy_request(request: &NetRequest) -> forge_policy::NetRequest {
    use forge_policy::HeaderValue;
    forge_policy::NetRequest {
        method: request.method.clone(),
        url: request.url.clone(),
        body_bytes: request.body.as_ref().map(|b| b.len() as u64),
        timeout_ms: request.timeout_ms,
        request_content_type: request.content_type.clone(),
        // Request headers projected onto the policy's header model: a literal
        // string maps to `HeaderValue::Literal` (the policy denies a secret-like
        // header carrying a literal value); a `{ secret_ref }` maps to
        // `HeaderValue::Secret`, which the policy permits ONLY when the matched
        // rule lists that header name in `allow_secret_headers`. So both the
        // literal-secret deny and the secret_ref allowlist gate run at the call
        // gate, before any secret is resolved or any request is sent (SC-13).
        headers: request
            .headers
            .iter()
            .map(|(k, v)| {
                let pv = match v {
                    NetHeaderValue::Literal(s) => HeaderValue::Literal(s.clone()),
                    NetHeaderValue::Secret { secret_ref } => {
                        HeaderValue::Secret { secret_ref: secret_ref.clone() }
                    }
                };
                (k.clone(), pv)
            })
            .collect(),
        ..Default::default()
    }
}

/// Whether `request` carries at least one `secret_ref` header (review 153). On a
/// response-leg denial the secret_ref headers have already been resolved and sent
/// over the wire (`resolve_secret_headers` ran inside the `host_call` closure), so
/// a secret DID cross the trust boundary and must still be audited as a
/// `secret.use`. This inspects the ORIGINAL (recorded-shape) request, whose headers
/// still carry `{ secret_ref }` (never a resolved value), so it leaks nothing.
fn request_carries_secret_ref(request: &NetRequest) -> bool {
    request
        .headers
        .values()
        .any(|v| matches!(v, NetHeaderValue::Secret { .. }))
}

/// Header names treated as secret-bearing for trace redaction. Mirrors the
/// policy's secret-header set (`policy/net.rs`); a *literal* value on one of
/// these must never be persisted into the trace, even on a denied request.
fn is_secret_header_name(name: &str) -> bool {
    const SECRET_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];
    let lower = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&lower.as_str())
}

/// Build the **trace-safe** recorded args for a `net.fetch` (SC-13). Starting
/// from the serialized request, every header whose value is a *literal* on a
/// secret-like header name is replaced with `"<redacted>"`, so a plaintext
/// secret the applet wrote as a literal never enters the RunRecord (even when the
/// request is denied and recorded as a denial). A `{ "secret_ref": "name" }`
/// header is left untouched — it carries only the non-sensitive ref, which the
/// trace is *required* to keep for replay/diagnostics. Non-header fields are
/// unchanged. This is purely the recorded shape; the bridge still receives the
/// original request and resolves/injects real values at the HTTP edge.
fn trace_safe_args(request: &NetRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(request).unwrap_or(serde_json::Value::Null);
    if let Some(headers) = value
        .get_mut("headers")
        .and_then(|h| h.as_object_mut())
    {
        for (name, hv) in headers.iter_mut() {
            // Only redact a LITERAL (a JSON string) on a secret-like header; a
            // secret_ref object stays so the trace keeps the ref.
            if hv.is_string() && is_secret_header_name(name) {
                *hv = serde_json::Value::String("<redacted>".to_string());
            }
        }
    }
    value
}

/// Whether a request body smuggles a secret reference (SC-13 exfil guard). M0a
/// only permits a `secret_ref` in a header value; one in the body is a
/// secret-exfil pattern and the request must be rejected before it is sent.
///
/// The body is opaque to the runtime (an already-serialized string), so this is
/// a conservative textual check: if the body parses as JSON and contains a
/// `secret_ref` object key anywhere, it is exfil; if it does not parse as JSON
/// but still contains the literal `"secret_ref"` token, it is treated as exfil
/// too (fail-closed). A benign body without the marker passes.
fn body_contains_secret_ref(body: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if json_has_secret_ref_key(&value) {
            return true;
        }
    }
    body.contains("\"secret_ref\"")
}

/// Recursively scan a JSON value for an object that has a `secret_ref` key.
fn json_has_secret_ref_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("secret_ref") || map.values().any(json_has_secret_ref_key)
        }
        serde_json::Value::Array(items) => items.iter().any(json_has_secret_ref_key),
        _ => false,
    }
}

/// Build the **request-phase** view of a net allowlist: the same rules in the
/// same order, but with each rule's *response* constraints (`max_response_bytes`,
/// `response_content_types`) cleared. The call gate checks against this view so a
/// rule that constrains the response cannot spuriously deny a request before its
/// response is known (the policy denies an unknown response content-type). The
/// response constraints are preserved in the full allowlist and enforced on the
/// response leg. All request-side fields (host/scheme/path/method/body/timeout/
/// request-content-type/secret-headers) are untouched, so the call gate's
/// request-side decision is identical to the full allowlist's.
pub(super) fn request_phase_allowlist(full: &NetGrant) -> NetGrant {
    NetGrant(
        full
            .rules()
            .iter()
            .map(|rule| forge_domain::NetRule {
                max_response_bytes: None,
                response_content_types: Vec::new(),
                ..rule.clone()
            })
            .collect(),
    )
}

/// Project the request **plus the real response** onto a [`forge_policy::NetRequest`]
/// for the **response-leg** egress check (SC-5 response caps + redirect/DNS facts).
/// This is the call-gate projection ([`to_policy_request`]) with the now-known
/// facts populated from the [`NetResponse`], so a second [`NetPolicy`] check matches
/// the **same** allowlist rule (host/scheme/path/method must still match) and
/// additionally enforces:
///   * `max_response_bytes` / `response_content_types` against the real body size
///     and content-type;
///   * the SC-5 redirect re-check against the `redirect_chain` the client actually
///     followed — every hop must independently satisfy a rule's request-side gates,
///     so a redirect to a private IP or an unallowlisted public host is denied;
///   * the SC-5 private-network deny against the `dns_answers` the host resolved —
///     a host resolving to a private literal address (DNS rebinding) is denied.
///
/// These redirect/DNS facts exist only on the response (they are products of the
/// transport, not the request), so they can only be checked here on the response
/// leg, never at the call gate. Reusing the same projection keeps the rule
/// selection identical to the call gate; only the response-derived facts are added.
fn to_response_policy_request(
    request: &NetRequest,
    response: &NetResponse,
) -> forge_policy::NetRequest {
    // Fold `final_url` (the URL the response actually came from) into the
    // policy-checked redirect hops so it is **bound to the allowlist** even when a
    // client reports it with an empty or truncated `redirect_chain` (review 074 #1).
    // Without this, a client that follows redirects and returns
    // `final_url = https://evil.example.net/...` but no chain would pass the
    // response-leg redirect check. Appending it (when it isn't already the last
    // hop) makes the policy re-run the full request-side gates against the real
    // final destination — fail-closed.
    let mut redirect_chain = response.redirect_chain.clone();
    if let Some(final_url) = &response.final_url {
        if redirect_chain.last() != Some(final_url) {
            redirect_chain.push(final_url.clone());
        }
    }
    forge_policy::NetRequest {
        response_bytes: Some(response.body_bytes()),
        response_content_type: response.content_type.clone(),
        redirect_chain,
        dns_answers: response.dns_answers.clone(),
        ..to_policy_request(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::MemoryHostBridge;
    use crate::host::HostContext;
    use crate::recorder::RunRecorder;
    use forge_domain::{ActorContext, Capabilities, Limits, Manifest, NetGrant, NetRule};

    fn manifest_with_net(net: NetGrant, max_host_calls: u64) -> Manifest {
        Manifest {
            entrypoint: "main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: Capabilities { net, ..Capabilities::default() },
            limits: Limits { max_host_calls, ..Limits::default() },
            compatibility: Default::default(),
        }
    }

    fn get_rule(url: &str) -> NetRule {
        NetRule { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    fn req(url: &str) -> NetRequest {
        NetRequest { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    /// An allowed fetch returns the bridge's (mock) response and records the call.
    #[test]
    fn net_fetch_allowed_records_and_serves_mock() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host.net_fetch(req("https://api.example.com/public/weather")).unwrap();
        assert_eq!(resp.status, 200);
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "net.fetch");
    }

    /// A non-allowlisted host is denied; the bridge is never touched and the
    /// denial is recorded as the run's `{"denied": …}` entry.
    #[test]
    fn net_fetch_denied_does_not_reach_bridge_and_is_recorded() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(req("https://evil.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        // The denial is in the trace as a recorded `net.fetch` denial.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "net.fetch");
        assert!(calls[0].response.get("denied").is_some());
        // And the bridge never saw a request.
        assert!(bridge.net_requests.is_empty());
    }

    /// An empty allowlist surfaces CapabilityRequired (category not declared).
    #[test]
    fn net_fetch_without_grant_is_capability_required() {
        let manifest = manifest_with_net(NetGrant::default(), 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(req("https://api.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        assert!(bridge.net_requests.is_empty());
    }

    // --- Response-leg SC-5 caps (max_response_bytes / response_content_types) --

    use crate::net::MockHttpClient;

    /// A rule with a response cap whose response, when it comes back, exceeds the
    /// cap is denied — the over-cap body is NOT served to the applet (SC-5
    /// max_response_bytes enforced on the response leg, not just at the call gate).
    #[test]
    fn net_fetch_oversized_response_is_denied_and_not_served() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(8);
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        // Inject a client whose response body is 16 bytes — over the 8-byte cap.
        let big = NetResponse {
            status: 200,
            body: Some("0123456789abcdef".into()), // 16 bytes > 8
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(big)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("response"), "{err}");
        assert!(err.to_string().contains("max_response_bytes"), "{err}");
    }

    /// A rule constraining response content-types denies a response whose
    /// content-type is outside the set (SC-5 response_content_types on the
    /// response leg). The wrong-type body never reaches the applet.
    #[test]
    fn net_fetch_wrong_response_content_type_is_denied() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        let html = NetResponse {
            status: 200,
            body: Some("<html></html>".into()),
            content_type: Some("text/html".into()),
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(html)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/page"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("content-type"), "{err}");
    }

    /// A response within the rule's caps is served unchanged: the new response-leg
    /// check must not over-deny a compliant response (positive control).
    #[test]
    fn net_fetch_response_within_caps_is_served() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(64);
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        // Default canned mock: 11-byte `{"ok":true}` JSON body, application/json.
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_deref(), Some(r#"{"ok":true}"#));
    }

    /// The response-leg cap is enforced on **replay** too: a recorded response
    /// that violates a response cap is denied identically when replayed (the
    /// recording is policy-bound; a tampered/oversized recording cannot smuggle a
    /// body past SC-5). We replay a recording whose `net.fetch` response is over
    /// the rule's cap and assert it surfaces the same PermissionDenied.
    #[test]
    fn net_fetch_response_cap_is_enforced_on_replay() {
        use crate::bridge::NullBridge;
        use forge_domain::RecordedCall;

        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(8);
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");

        // A recorded trace whose net.fetch response body is 16 bytes (> 8 cap).
        let recorded_resp = NetResponse {
            status: 200,
            body: Some("0123456789abcdef".into()),
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let request = req("https://api.example.com/public/weather");
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "net.fetch".into(),
            args: serde_json::to_value(&request).unwrap(),
            response: serde_json::to_value(&recorded_resp).unwrap(),
        }];

        // Replay must NOT touch a live bridge; NullBridge proves the recorder
        // serves the response, and the response-leg policy check still denies it.
        let mut bridge = NullBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, recorded),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("max_response_bytes"), "{err}");
    }

    /// A rule that constrains the response content-type still denies a
    /// request-side violation (wrong host) AT THE CALL GATE — before any request
    /// reaches the client. Proves the request-phase allowlist (response
    /// constraints stripped) keeps every request-side gate intact: the response
    /// caps don't make the call gate either over-deny a good host or under-deny a
    /// bad one.
    #[test]
    fn net_fetch_response_constrained_rule_still_gates_request_side() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(1024);
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        // Wrong host: denied at the call gate, bridge never touched.
        let err = host.net_fetch(req("https://evil.example.com/public/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(bridge.net_requests.is_empty(), "denied request must not reach the client");
    }

    // --- Response-leg SC-5 redirect / DNS facts (review 070 P1 #2) -----------

    /// A redirect whose every hop (origin + final) is allowlisted is allowed: the
    /// client reports the chain on the response and the response-leg check
    /// re-validates each hop, all of which pass.
    #[test]
    fn net_fetch_redirect_through_allowlisted_hop_is_allowed() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![
                get_rule("https://api.example.com/public/*"),
                get_rule("https://cdn.example.com/public/*"),
            ]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // The mock simulates a 302 chain api -> cdn, both allowlisted.
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/asset".into(),
                "https://cdn.example.com/public/asset".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.redirect_chain.len(), 2);
    }

    /// A redirect whose final hop is a private IP is denied AFTER the fetch: the
    /// hop is on the response (not the request), so the call gate could not catch
    /// it; the response-leg check denies it and the body never reaches the applet.
    #[test]
    fn net_fetch_redirect_to_private_ip_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/redirect".into(),
                "http://127.0.0.1/admin".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/redirect"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
    }

    /// A redirect to a public-but-unallowlisted host is denied after the fetch:
    /// the hop is not private, but its origin is not in the allowlist, so the
    /// per-hop re-check on the response leg denies it (SC-5 redirect re-check).
    #[test]
    fn net_fetch_redirect_to_unallowlisted_public_host_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/asset".into(),
                "https://evil.example.net/public/asset".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("redirect hop"), "{err}");
    }

    /// Review 074 #1: a client that lands on an unallowlisted final URL but reports
    /// an EMPTY `redirect_chain` (only `final_url`) must still be denied — the
    /// response leg folds `final_url` into the policy-checked hops.
    #[test]
    fn net_fetch_final_url_to_unallowlisted_host_is_denied_without_a_chain() {
        use crate::net::{MockHttpClient, NetResponse};
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // Followed a redirect to evil but reports NO chain — only the final_url.
        let response = NetResponse {
            status: 200,
            body: Some(r#"{"ok":true}"#.into()),
            content_type: Some("application/json".into()),
            final_url: Some("https://evil.example.net/public/asset".into()),
            redirect_chain: vec![],
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(response)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "final_url must be policy-bound: {err}");
    }

    /// A host that resolves (dns_answers) to a private IP is denied after the
    /// fetch (DNS rebinding): the request URL host is public, so the call gate
    /// allowed it, but the resolved literal address is private and the
    /// response-leg check denies it before the body reaches the applet.
    #[test]
    fn net_fetch_dns_rebinding_to_private_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // Public request host, but it resolves to a loopback literal.
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(
            MockHttpClient::with_dns_answers(vec!["127.0.0.1".into()]),
        ));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
        assert!(err.to_string().contains("DNS answer"), "{err}");
    }

    /// The allowed redirect case records byte-identically and replays unchanged:
    /// record the run, then replay the recorded trace through a NullBridge (no
    /// live network) and assert the served response is identical (redirect/DNS
    /// facts ride on the recording too).
    #[test]
    fn net_fetch_allowed_redirect_replays_byte_identical() {
        use crate::bridge::NullBridge;
        use crate::net::MockHttpClient;

        let manifest = manifest_with_net(
            NetGrant(vec![
                get_rule("https://api.example.com/public/*"),
                get_rule("https://cdn.example.com/public/*"),
            ]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let request = req("https://api.example.com/public/asset");

        // Record.
        let mut rec_bridge = MemoryHostBridge::with_http_client(Box::new(
            MockHttpClient::with_redirects(vec![
                "https://api.example.com/public/asset".into(),
                "https://cdn.example.com/public/asset".into(),
            ]),
        ));
        let mut rec_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut rec_bridge,
        )
        .unwrap();
        let recorded_resp = rec_host.net_fetch(request.clone()).unwrap();
        let (recorder, _logs) = rec_host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);

        // Replay the recorded trace; NullBridge proves no live network is touched
        // and the response-leg policy check still passes for the allowed chain.
        let mut replay_bridge = NullBridge::new();
        let mut replay_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, calls),
            &mut replay_bridge,
        )
        .unwrap();
        let replayed_resp = replay_host.net_fetch(request).unwrap();
        assert_eq!(recorded_resp, replayed_resp);
        assert_eq!(replayed_resp.redirect_chain.len(), 2);
    }

    /// `net.fetch` counts against the host-call flood cap (SC-2): the (n+1)th
    /// allowed fetch over `max_host_calls` trips ResourceLimitExceeded.
    #[test]
    fn net_fetch_counts_against_host_call_budget() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            1,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        assert!(host.net_fetch(req("https://api.example.com/public/a")).is_ok());
        let err = host
            .net_fetch(req("https://api.example.com/public/b"))
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
    }

    // --- SC-13: ctx.secrets header injection ---------------------------------

    use crate::net::{InMemorySecretStore, NetHeaderValue};

    /// A GET rule that allows a `secret_ref` into the named header.
    fn secret_rule(url: &str, header: &str) -> NetRule {
        NetRule {
            method: "GET".into(),
            url: url.into(),
            allow_secret_headers: vec![header.into()],
            ..Default::default()
        }
    }

    /// A GET request carrying a `secret_ref` in `header`.
    fn secret_req(url: &str, header: &str, secret_ref: &str) -> NetRequest {
        let mut r = req(url);
        r.headers.insert(
            header.into(),
            NetHeaderValue::Secret { secret_ref: secret_ref.into() },
        );
        r
    }

    /// An allowlisted secret header is injected: a capturing client sees the
    /// RESOLVED value, but the RunRecord trace + the applet's returned response
    /// carry only the secret_ref, never the plaintext (SC-13).
    #[test]
    fn secret_header_is_injected_into_client_but_not_trace_or_return() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "Authorization",
                "tok",
            ))
            .unwrap();

        // The applet's returned response carries no secret value.
        let resp_json = serde_json::to_string(&resp).unwrap();
        assert!(!resp_json.contains("SECRET-XYZ"), "applet return leaked the secret: {resp_json}");

        // The RECORDED trace keeps only the secret_ref — never the value.
        let (recorder, logs) = host.finish();
        let calls = recorder.into_calls();

        // The CLIENT received the resolved literal header value (injection happened
        // at the HTTP edge). Read after `finish()` releases the &mut bridge borrow.
        assert_eq!(bridge.net_requests.len(), 1);
        let sent = &bridge.net_requests[0];
        assert_eq!(
            sent.headers.get("Authorization").and_then(|h| h.as_literal()),
            Some("Bearer SECRET-XYZ"),
            "client must receive the RESOLVED secret value"
        );

        let trace = serde_json::to_string(&calls).unwrap();
        assert!(trace.contains("secret_ref"), "trace must keep the secret_ref: {trace}");
        assert!(trace.contains("\"tok\""), "trace must keep the ref name: {trace}");
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        // And no log line carries it either.
        assert!(!logs.join("\n").contains("SECRET-XYZ"), "logs leaked the secret");
    }

    /// A secret_ref on a header the matched rule does NOT allowlist is denied at
    /// the call gate; nothing reaches the client and no value is resolved.
    #[test]
    fn secret_header_not_allowlisted_is_denied_before_send() {
        // Rule allowlists "Authorization" but the request uses "X-Api-Key".
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "X-Api-Key",
                "tok",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        assert!(bridge.net_requests.is_empty(), "denied secret header must not send");
    }

    /// An allowlisted header whose secret NAME is unknown to the store is a
    /// fail-closed RuntimeError; nothing is sent and the value never exists.
    #[test]
    fn unknown_secret_name_is_runtime_error_and_sends_nothing() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // The store is EMPTY: the named secret cannot be resolved.
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            InMemorySecretStore::new(),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "Authorization",
                "missing",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        assert!(err.to_string().contains("missing"), "error names the ref: {err}");
        drop(host); // release the &mut bridge borrow before reading the bridge
        assert!(
            bridge.net_requests.is_empty(),
            "an unresolvable secret must not reach the client"
        );
    }

    /// A `secret_ref` smuggled into the request BODY is rejected as a
    /// ValidationError (secret-exfil guard) before any policy/bridge work.
    #[test]
    fn secret_ref_in_body_is_rejected() {
        let manifest = manifest_with_net(
            NetGrant(vec![NetRule {
                method: "POST".into(),
                url: "https://api.example.com/report".into(),
                ..Default::default()
            }]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let request = NetRequest {
            method: "POST".into(),
            url: "https://api.example.com/report".into(),
            body: Some(r#"{"token":{"secret_ref":"tok"}}"#.into()),
            ..Default::default()
        };
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        drop(host); // release the &mut bridge borrow before reading the bridge
        assert!(bridge.net_requests.is_empty(), "a body secret_ref must not send");
    }

    /// A *literal* value on a secret-like header is denied by policy AND redacted
    /// from the recorded trace, so the plaintext never persists even on a denial.
    #[test]
    fn literal_secret_header_is_redacted_from_the_trace() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let mut request = req("https://api.example.com/private/me");
        request.headers.insert(
            "Authorization".into(),
            NetHeaderValue::Literal("Bearer LITERAL-LEAK".into()),
        );
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        assert!(!trace.contains("LITERAL-LEAK"), "literal secret leaked into trace: {trace}");
        assert!(trace.contains("<redacted>"), "secret-like literal must be redacted: {trace}");
    }

    /// A response-leg denial (e.g. a redirect to an unallowlisted host) after a
    /// secret was injected must NOT persist the rejected response body or the
    /// secret value; the recorded entry is redacted to a denial (review 074 #2).
    #[test]
    fn response_leg_denial_after_injection_is_trace_safe() {
        use crate::net::NetResponse;
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        // The transport followed a redirect to an UNALLOWLISTED host and returns a
        // body that must not be recorded once the response-leg policy denies.
        let transport = NetResponse {
            status: 200,
            body: Some("REJECTED-BODY".into()),
            content_type: Some("application/json".into()),
            final_url: Some("https://evil.example/collect".into()),
            redirect_chain: vec![
                "https://api.example.com/private/redirect".into(),
                "https://evil.example/collect".into(),
            ],
            ..Default::default()
        };
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::new(transport)),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/redirect",
                "Authorization",
                "tok",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        let trace = serde_json::to_string(&calls).unwrap();
        // The secret_ref survives (the request was recorded) but neither the
        // resolved value nor the rejected response body persists.
        assert!(trace.contains("secret_ref"), "trace must keep the secret_ref: {trace}");
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        assert!(!trace.contains("REJECTED-BODY"), "trace persisted the rejected body: {trace}");
        // Review 153: the response-leg denial that ALREADY injected the secret stamps
        // the non-sensitive `secret_injected` marker on the redacted response, so the
        // audit builder can still mint the `secret.use` row. The marker is a bare
        // boolean — it carries no secret value.
        assert_eq!(calls.len(), 1);
        let response = &calls[0].response;
        assert!(response.get("denied").is_some(), "denied-shaped: {response}");
        assert_eq!(
            response.get("secret_injected").and_then(|v| v.as_bool()),
            Some(true),
            "an injected secret must be marked on the response-leg denial: {response}"
        );
        assert!(response.get("status").is_none(), "denial shape has no status: {response}");
    }

    /// Review 153: a request-gate denial (the secret_ref header is NOT allowlisted)
    /// never resolves or sends the secret, so its redacted response carries NO
    /// `secret_injected` marker — the audit builder must NOT mint a `secret.use` row
    /// for it. This guards the marker against false positives on the request-gate
    /// denial path (which uses `record_denial`, never `redact_last_response`).
    #[test]
    fn request_gate_denial_does_not_mark_secret_injected() {
        // The rule allowlists "Authorization" but the request smuggles the secret on
        // a NON-allowlisted header, so the call gate denies before any send.
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req("https://api.example.com/private/me", "X-Api-Key", "tok"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        let response = &calls[0].response;
        assert!(response.get("denied").is_some(), "denied-shaped: {response}");
        assert!(
            response.get("secret_injected").is_none(),
            "a request-gate denial sent nothing — no secret_injected marker: {response}"
        );
    }
}
