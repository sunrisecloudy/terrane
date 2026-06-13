//! Data-driven T011 network-egress vectors (`forge/fixtures/network/`).
//!
//! Each `<case>.json` carries an `allowlist` (a list of SC-8 net rules → a
//! [`NetGrant`]) and a `request` (→ [`NetRequest`]) plus an `expect`
//! (`allow`/`deny`). We deserialize both straight into the domain/policy types
//! and assert [`NetPolicy::check`] agrees with `expect` — this is the executable
//! contract for prd-merged/07 SC-5/SC-8.
//!
//! ## Runtime-DNS vectors are asserted separately
//!
//! `manifest.json` lists `runtime_dns_required` — vectors whose *authoritative*
//! decision needs a live DNS resolver / redirect follower (a runtime concern,
//! not this pure crate). `NetPolicy` still makes a best-effort *literal* check
//! for these (it re-checks every redirect hop and every literal DNS answer for a
//! private literal IP), and for the three T011 runtime-DNS vectors the decisive
//! signal happens to be a literal IP, so the literal engine reaches the same
//! verdict. We assert those in a clearly-labelled second pass so the boundary
//! between "decided here" and "needs the runtime resolver" stays explicit.

use forge_domain::NetGrant;
use forge_policy::{NetPolicy, NetRequest};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/policy
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/network")
        .canonicalize()
        .expect("network fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

/// Deserialize a vector's `allowlist` + `request` and run the policy check.
fn check_vector(vector: &serde_json::Value) -> forge_domain::Result<()> {
    let allowlist: NetGrant = serde_json::from_value(vector["allowlist"].clone())
        .expect("allowlist deserializes into NetGrant");
    let request: NetRequest = serde_json::from_value(vector["request"].clone())
        .expect("request deserializes into NetRequest");
    NetPolicy::new(&allowlist).check(&request)
}

/// Assert one vector matches its `expect`, returning a label for messages.
fn assert_vector(case: &str, vector: &serde_json::Value) {
    let expect = vector["expect"].as_str().expect("vector has expect");
    let reason = vector.get("reason").and_then(|r| r.as_str()).unwrap_or("");
    let result = check_vector(vector);
    match expect {
        "allow" => assert!(
            result.is_ok(),
            "case {case}: expected ALLOW ({reason}) but got {:?}",
            result.unwrap_err()
        ),
        "deny" => {
            let err = result.expect_err(&format!(
                "case {case}: expected DENY ({reason}) but the request was allowed"
            ));
            // A denial is either "no net capability" (CapabilityRequired) or a
            // covered-but-refused request (PermissionDenied). Every T011 deny
            // vector declares a non-empty allowlist, so it must be the latter.
            assert_eq!(
                err.code(),
                "PermissionDenied",
                "case {case}: deny should be PermissionDenied ({reason}), got {err}"
            );
        }
        other => panic!("case {case}: unknown expect {other:?}"),
    }
}

/// The manifest's declared case list + its `runtime_dns_required` set.
fn suite() -> serde_json::Value {
    load("manifest.json")
}

#[test]
fn t011_literal_vectors_match_expect() {
    let suite = suite();
    let runtime_dns: Vec<String> = suite["runtime_dns_required"]
        .as_array()
        .expect("runtime_dns_required is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let cases = suite["cases"].as_array().expect("cases is an array");
    let mut checked = 0usize;
    for entry in cases {
        let case = entry["case"].as_str().expect("case name");
        // Skip the runtime-DNS vectors here; they are asserted separately below.
        let requires_dns = entry
            .get("requires_runtime_dns")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || runtime_dns.iter().any(|c| c == case);
        if requires_dns {
            continue;
        }
        let file = entry["file"].as_str().expect("file name");
        let vector = load(file);
        assert_vector(case, &vector);
        checked += 1;
    }
    // 24 total − 3 runtime-DNS = 21 literal vectors.
    assert_eq!(checked, 21, "expected 21 literal vectors, checked {checked}");
}

#[test]
fn t011_runtime_dns_vectors_handled_best_effort() {
    // The three `runtime_dns_required` vectors. Their *authoritative* decision is
    // a runtime resolver concern, but each one's decisive signal in T011 is a
    // literal IP (a private literal redirect hop / DNS answer, or an all-public
    // allowlisted redirect chain), so the literal engine reaches the documented
    // verdict. This keeps the literal/runtime boundary explicit and asserted.
    let suite = suite();
    let runtime_dns: Vec<String> = suite["runtime_dns_required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(runtime_dns.len(), 3, "T011 declares 3 runtime-DNS vectors");

    let cases = suite["cases"].as_array().unwrap();
    for entry in cases {
        let case = entry["case"].as_str().unwrap();
        if !runtime_dns.iter().any(|c| c == case) {
            continue;
        }
        let vector = load(entry["file"].as_str().unwrap());
        assert_vector(case, &vector);
    }
}

#[test]
fn every_listed_case_file_exists_and_count_is_24() {
    let suite = suite();
    assert_eq!(suite["count"].as_u64(), Some(24), "manifest count is 24");
    let cases = suite["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 24, "24 case entries");
    for entry in cases {
        let file = entry["file"].as_str().unwrap();
        let path = fixtures_dir().join(file);
        assert!(path.exists(), "fixture file missing: {}", path.display());
    }
}
