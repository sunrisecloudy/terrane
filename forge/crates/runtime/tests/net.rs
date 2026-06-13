//! `ctx.net.fetch` end-to-end: egress policy gate + deterministic record/replay.
//!
//! prd-merged/07 SC-5 (network egress policy), SC-8 (net grammar); prd-merged/01
//! CR-3 (`net` namespace), CR-4 (call-time capability check), CR-8 (deterministic
//! mode: live network forbidden unless a recorded response is being served).
//!
//! NO live network runs here: the [`MemoryHostBridge`] forwards `ctx.net.fetch`
//! to an injected [`MockHttpClient`] (a canned response, never a socket), and
//! replay serves the recorded response without touching the bridge at all.

mod common;

use common::{net_manifest, owner, program, spine_manifest, viewer};
use forge_domain::RunOutcome;
use forge_runtime::{
    record_run, replay, MemoryHostBridge, MockHttpClient, NetResponse, NullBridge,
};

/// An applet that fetches an ALLOWED url and returns the response body. The mock
/// client serves a canned `{ "temp": 21 }`; the call is recorded.
fn fetch_applet() -> forge_runtime::Program {
    program(
        r#"export async function main(ctx, input) {
            const resp = await ctx.net.fetch({
                method: "GET",
                url: "https://api.example.com/public/weather"
            });
            return { ok: true, value: { status: resp.status, body: resp.body } };
        }"#,
    )
}

/// A bridge whose injected client returns a specific response, so the test can
/// assert the applet got the mock's bytes (not a real network read).
fn bridge_with_canned() -> MemoryHostBridge {
    MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(NetResponse {
        status: 200,
        body: Some(r#"{"temp":21}"#.to_string()),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    })))
}

#[test]
fn allowed_fetch_gets_mock_response_and_is_recorded() {
    let prog = fetch_applet();
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &net_manifest(),
        &owner(),
        &serde_json::json!({}),
        42,
        1000,
        &mut bridge,
    )
    .unwrap();

    // The applet saw the mock client's response (status + body).
    assert!(record.is_completed(), "record outcome: {:?}", record.outcome);
    match &record.outcome {
        RunOutcome::Completed { result } => {
            assert_eq!(result.value["status"], serde_json::json!(200));
            assert_eq!(result.value["body"], serde_json::json!(r#"{"temp":21}"#));
        }
        other => panic!("expected completion, got {other:?}"),
    }

    // Exactly one request reached the client (the allowed fetch), and the call is
    // captured in the deterministic trace as `net.fetch`.
    assert_eq!(bridge.net_requests.len(), 1);
    assert_eq!(bridge.net_requests[0].url, "https://api.example.com/public/weather");
    assert!(
        record.calls.iter().any(|c| c.method == "net.fetch"),
        "net.fetch must be in the recorded trace: {:?}",
        record.calls
    );
}

#[test]
fn recorded_fetch_replays_identically_without_touching_the_client() {
    let prog = fetch_applet();
    let mut bridge = bridge_with_canned();
    let original = record_run(
        &prog,
        &net_manifest(),
        &owner(),
        &serde_json::json!({}),
        42,
        1000,
        &mut bridge,
    )
    .unwrap();

    // Replay against a NullBridge: it refuses every live effect, so if replay
    // tried a live net.fetch the run would fail. It must NOT — the recorder
    // serves the recorded response (CR-8).
    let mut null = NullBridge::new();
    let replayed = replay(&original, &prog, &net_manifest(), &owner(), &mut null).unwrap();

    assert!(
        original.replays_identically(&replayed),
        "net.fetch must replay byte-identically:\n original={:?}\n replayed={:?}",
        original.calls,
        replayed.calls
    );
    assert!(replayed.is_completed());
}

#[test]
fn non_allowlisted_url_is_denied_and_no_request_reaches_the_client() {
    // The applet fetches a host the manifest does NOT allowlist.
    let prog = program(
        r#"export async function main(ctx, input) {
            const resp = await ctx.net.fetch({
                method: "GET",
                url: "https://evil.example.com/steal"
            });
            return { ok: true, value: resp };
        }"#,
    );
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &net_manifest(),
        &owner(),
        &serde_json::json!({}),
        7,
        0,
        &mut bridge,
    )
    .unwrap();

    // The run failed with a permission denial (the host is not allowlisted).
    match &record.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected a PermissionDenied failure, got {other:?}"),
    }
    // CRITICAL: no request ever reached the HTTP client.
    assert!(
        bridge.net_requests.is_empty(),
        "a denied fetch must not reach the client: {:?}",
        bridge.net_requests
    );
}

#[test]
fn fetch_without_any_net_grant_is_capability_required() {
    // The spine manifest declares no `net` rule at all → CapabilityRequired,
    // distinct from a declared-but-unmatched rule (PermissionDenied above).
    let prog = fetch_applet();
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();

    match &record.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "CapabilityRequired"),
        other => panic!("expected CapabilityRequired, got {other:?}"),
    }
    assert!(bridge.net_requests.is_empty());
}

#[test]
fn private_ip_url_is_denied_by_default() {
    // Even if a rule listed it, SC-5 denies private/loopback targets. Here the
    // applet targets a literal loopback IP; the deny fires before any send.
    let prog = program(
        r#"export async function main(ctx, input) {
            const resp = await ctx.net.fetch({
                method: "GET",
                url: "http://127.0.0.1/admin"
            });
            return { ok: true, value: resp };
        }"#,
    );
    // Allowlist literally lists loopback to prove the private-network deny wins.
    let mut manifest = net_manifest();
    manifest.capabilities.net = forge_domain::NetGrant(vec![forge_domain::NetRule {
        method: "GET".into(),
        url: "http://127.0.0.1/*".into(),
        ..Default::default()
    }]);
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &manifest,
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();

    match &record.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "PermissionDenied");
            assert!(
                error.to_string().contains("private"),
                "denial must cite the private-network rule: {error}"
            );
        }
        other => panic!("expected a private-network denial, got {other:?}"),
    }
    assert!(bridge.net_requests.is_empty(), "private target must never be sent");
}

#[test]
fn localhost_hostname_is_denied_by_default() {
    let prog = program(
        r#"export async function main(ctx, input) {
            return await ctx.net.fetch({ method: "GET", url: "http://localhost/x" });
        }"#,
    );
    let mut manifest = net_manifest();
    manifest.capabilities.net = forge_domain::NetGrant(vec![forge_domain::NetRule {
        method: "GET".into(),
        url: "http://localhost/*".into(),
        ..Default::default()
    }]);
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &manifest,
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match &record.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected localhost denial, got {other:?}"),
    }
    assert!(bridge.net_requests.is_empty());
}

#[test]
fn scheme_downgrade_to_http_is_denied() {
    // The rule is https; an http request may not satisfy it (no downgrade).
    let prog = program(
        r#"export async function main(ctx, input) {
            return await ctx.net.fetch({ method: "GET", url: "http://api.example.com/public/x" });
        }"#,
    );
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &net_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match &record.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected scheme-downgrade denial, got {other:?}"),
    }
    assert!(bridge.net_requests.is_empty());
}

#[test]
fn viewer_role_cannot_fetch() {
    // SC-10: a read-only role cannot run applets, so it cannot fetch either.
    let prog = fetch_applet();
    let mut bridge = bridge_with_canned();
    let record = record_run(
        &prog,
        &net_manifest(),
        &viewer(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match &record.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected a role denial, got {other:?}"),
    }
    assert!(bridge.net_requests.is_empty());
}
