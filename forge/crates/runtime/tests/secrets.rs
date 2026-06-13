//! Data-driven `ctx.secrets` injection vectors (`forge/fixtures/secrets/`).
//!
//! prd-merged/07 **SC-13/SC-12** (secret injection at the HTTP edge, never
//! readable by the applet / trace / log / sync), prd-merged/01 **CR-3** (`net`),
//! **CR-8** (deterministic replay). The executable contract for `spec/secrets.md`.
//!
//! NORMATIVE invariant proven here: an applet references a secret **by name**
//! (`{ "secret_ref": "..." }`); the host injects the resolved value into the
//! OUTGOING request handed to the [`HttpClient`] for an allowlisted destination
//! ONLY; that value NEVER appears in the recorded `net.fetch` args, the response
//! the applet receives, or any log. The trace keeps only the `secret_ref`.
//!
//! Each `<case>.json` carries `secrets` + `allowlist` + `request` + an `expect`
//! (`injected`/`allow`/`trace_safe`/`deny`/`error`/`reject`/`deny_trace_safe`).
//! We drive [`HostContext::net_fetch`] directly so the test sees exactly what the
//! bridge/client received (the resolved request) AND what the recorder captured
//! (the secret_ref). NO live network: the bridge forwards to an injected mock.

use forge_domain::{ActorContext, Capabilities, Limits, Manifest, NetGrant};
use forge_runtime::{
    HostContext, HttpClient, InMemorySecretStore, MemoryHostBridge, MockHttpClient, NetRequest,
    NetResponse, RunRecorder,
};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/runtime
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/secrets")
        .canonicalize()
        .expect("secrets fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

/// Build a manifest whose `net` allowlist is the fixture's `allowlist`.
fn manifest_for(allowlist: &serde_json::Value) -> Manifest {
    let net: NetGrant =
        serde_json::from_value(allowlist.clone()).expect("allowlist → NetGrant");
    Manifest {
        entrypoint: "main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities: Capabilities { net, ..Capabilities::default() },
        limits: Limits { max_host_calls: 100, ..Limits::default() },
    }
}

/// Build the runtime `NetRequest` from the fixture's `request`. The fixture body,
/// when present, is a JSON object (the applet's structured payload) — serialize
/// it to the opaque string the runtime carries so the secret-exfil body guard
/// sees it exactly as the engine would.
fn request_for(request: &serde_json::Value) -> NetRequest {
    let mut obj = request.as_object().cloned().expect("request is an object");
    if let Some(body) = obj.get("body") {
        if !body.is_string() {
            let serialized = serde_json::to_string(body).expect("serialize fixture body");
            obj.insert("body".into(), serde_json::Value::String(serialized));
        }
    }
    serde_json::from_value(serde_json::Value::Object(obj)).expect("request → NetRequest")
}

/// Build the secret store from the fixture's `secrets` map.
fn secrets_for(secrets: &serde_json::Value) -> InMemorySecretStore {
    let mut store = InMemorySecretStore::new();
    if let Some(map) = secrets.as_object() {
        for (name, value) in map {
            store = store.with_secret(name.clone(), value.as_str().unwrap_or_default());
        }
    }
    store
}

/// The mock client the bridge forwards to. For the redirect/trace-safe case the
/// fixture supplies a `transport_response`; otherwise a canned 200 JSON.
fn client_for(vector: &serde_json::Value) -> Box<dyn HttpClient> {
    match vector.get("transport_response") {
        Some(tr) => {
            let resp: NetResponse =
                serde_json::from_value(tr.clone()).expect("transport_response → NetResponse");
            Box::new(MockHttpClient::new(resp))
        }
        None => Box::new(MockHttpClient::canned()),
    }
}

/// Run one vector end-to-end through `HostContext::net_fetch`, returning the
/// fetch result, the bridge (whose `net_requests` captured what the CLIENT got),
/// and the recorded trace (what the RUN RECORD persists).
struct Outcome {
    result: forge_domain::Result<NetResponse>,
    client_saw: Vec<NetRequest>,
    trace_json: String,
}

fn run_vector(vector: &serde_json::Value) -> Outcome {
    let manifest = manifest_for(&vector["allowlist"]);
    let request = request_for(&vector["request"]);
    let secrets = secrets_for(&vector["secrets"]);
    let client = client_for(vector);

    let actor = ActorContext::owner("dev");
    let mut bridge =
        MemoryHostBridge::with_http_and_secrets(client, secrets);
    let mut host = HostContext::new(
        &manifest,
        &actor,
        RunRecorder::recording(1, 0),
        &mut bridge,
    )
    .expect("host context");
    let result = host.net_fetch(request);
    let (recorder, _logs) = host.finish();
    let calls = recorder.into_calls();
    let trace_json = serde_json::to_string(&calls).expect("serialize trace");
    let client_saw = std::mem::take(&mut bridge.net_requests);
    Outcome { result, client_saw, trace_json }
}

/// Assert no listed secret value appears anywhere in `haystack`.
fn assert_absent(haystack: &str, must_not_contain: &[serde_json::Value], label: &str) {
    for needle in must_not_contain {
        let s = needle.as_str().expect("must_not_contain entries are strings");
        assert!(
            !haystack.contains(s),
            "{label}: secret value {s:?} must NOT appear in {haystack}"
        );
    }
}

#[test]
fn secrets_manifest_count_matches_files() {
    let manifest = load("manifest.json");
    let count = manifest["count"].as_u64().expect("count");
    let cases = manifest["cases"].as_array().expect("cases");
    assert_eq!(count as usize, cases.len(), "manifest count vs cases");
}

#[test]
fn all_secret_vectors() {
    let manifest = load("manifest.json");
    let cases = manifest["cases"].as_array().expect("cases");
    for case in cases {
        let file = case["file"].as_str().expect("case file");
        let name = case["case"].as_str().expect("case name");
        let vector = load(file);
        assert_vector(name, &vector);
    }
}

fn assert_vector(case: &str, vector: &serde_json::Value) {
    let expect = vector["expect"].as_str().expect("expect");
    let reason = vector.get("reason").and_then(|r| r.as_str()).unwrap_or("");
    let out = run_vector(vector);

    match expect {
        // The secret header is injected: the fetch succeeds, the CLIENT received
        // the resolved literal value, and the TRACE keeps only the secret_ref.
        "injected" | "allow" | "trace_safe" => {
            assert!(
                out.result.is_ok(),
                "case {case}: expected success ({reason}) but got {:?}",
                out.result.unwrap_err()
            );

            // The client saw the resolved header value (proves injection at the edge).
            if let Some(injected) = vector.get("injected_header").and_then(|h| h.as_object()) {
                assert_eq!(out.client_saw.len(), 1, "case {case}: one request reached the client");
                let sent = &out.client_saw[0];
                for (header, value) in injected {
                    let want = value.as_str().expect("injected header value");
                    let got = sent
                        .headers
                        .get(header)
                        .and_then(|hv| hv.as_literal())
                        .unwrap_or_else(|| panic!("case {case}: header {header:?} missing/secret on client request"));
                    assert_eq!(got, want, "case {case}: client must receive the RESOLVED header value");
                }
            }

            // TRACE-SAFETY: the recorded args carry the secret_ref, never the value.
            if let Some(must_contain) = vector
                .get("trace_expectation")
                .and_then(|t| t.get("headers"))
                .or_else(|| vector.get("trace_must_contain"))
            {
                // The secret_ref object appears (serialized) in the trace.
                let needle = serde_json::to_string(must_contain).unwrap();
                let inner = needle.trim_start_matches('{').trim_end_matches('}');
                assert!(
                    out.trace_json.contains(inner) || out.trace_json.contains("secret_ref"),
                    "case {case}: trace must keep the secret_ref ({inner}) — trace={}",
                    out.trace_json
                );
            }
            let must_not = collect_must_not_contain(vector);
            assert_absent(&out.trace_json, &must_not, &format!("case {case} (trace)"));
            // The applet's returned response must not carry the secret either.
            if let Ok(resp) = &out.result {
                let resp_json = serde_json::to_string(resp).unwrap();
                assert_absent(&resp_json, &must_not, &format!("case {case} (applet return)"));
            }
        }

        // A policy denial: the run fails with the expected code and NO request
        // reaches the client; the secret value never appears in the trace.
        "deny" | "deny_trace_safe" => {
            let err = out
                .result
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("case {case}: expected a denial ({reason}), got Ok"));
            if let Some(code) = vector.get("expected_error_code").and_then(|c| c.as_str()) {
                assert_eq!(err.code(), code, "case {case}: denial code");
            }
            // deny → never reaches the client; deny_trace_safe → the request may be
            // sent but the response-leg denies, so the CLIENT could have seen it.
            if vector
                .get("client_must_not_receive_request")
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
            {
                assert!(
                    out.client_saw.is_empty(),
                    "case {case}: a denied fetch must not reach the client: {:?}",
                    out.client_saw
                );
            }
            // The secret value (and any rejected body) must not be in the trace.
            let must_not = collect_must_not_contain(vector);
            assert_absent(&out.trace_json, &must_not, &format!("case {case} (trace)"));
            // deny_trace_safe additionally requires the secret_ref to survive in the
            // trace (the request was recorded) while the value/body did not.
            if expect == "deny_trace_safe" {
                assert!(
                    out.trace_json.contains("secret_ref"),
                    "case {case}: trace must keep the secret_ref — trace={}",
                    out.trace_json
                );
            }
        }

        // An unresolvable secret name (unknown/revoked): RuntimeError, nothing sent.
        "error" => {
            let err = out
                .result
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("case {case}: expected an error ({reason}), got Ok"));
            if let Some(code) = vector.get("expected_error_code").and_then(|c| c.as_str()) {
                assert_eq!(err.code(), code, "case {case}: error code");
            }
            assert!(
                out.client_saw.is_empty(),
                "case {case}: an unresolvable secret must not send: {:?}",
                out.client_saw
            );
            let must_not = collect_must_not_contain(vector);
            assert_absent(&out.trace_json, &must_not, &format!("case {case} (trace)"));
        }

        // A secret_ref smuggled into the request body: ValidationError, nothing sent.
        "reject" => {
            let err = out
                .result
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("case {case}: expected a rejection ({reason}), got Ok"));
            if let Some(code) = vector.get("expected_error_code").and_then(|c| c.as_str()) {
                assert_eq!(err.code(), code, "case {case}: rejection code");
            }
            assert!(
                out.client_saw.is_empty(),
                "case {case}: a body secret_ref must not send: {:?}",
                out.client_saw
            );
        }

        other => panic!("case {case}: unknown expect {other:?}"),
    }
}

/// Gather every `trace_must_not_contain` / `must_not_contain` list a vector may
/// carry (top-level and nested under `trace_expectation`).
fn collect_must_not_contain(vector: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Some(arr) = vector.get("trace_must_not_contain").and_then(|v| v.as_array()) {
        out.extend(arr.iter().cloned());
    }
    if let Some(arr) = vector
        .get("trace_expectation")
        .and_then(|t| t.get("must_not_contain"))
        .and_then(|v| v.as_array())
    {
        out.extend(arr.iter().cloned());
    }
    out
}
