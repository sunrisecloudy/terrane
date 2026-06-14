//! Data-driven SC-10 seven-gate vectors (`forge/fixtures/policy-gates/`, T037).
//!
//! Each `<case>.json` carries a host-call `request`, an `actor` role, a trusted
//! `manifest` (capability grants + `max_host_calls`), and the three **trusted**
//! gate inputs — `workspace_policy`, `run_profile`, `platform` — plus the
//! `expect`ed decision (which gate, allow/deny, error code, reason substring).
//!
//! We build a real [`ComposedDecisionContext`] from the trusted inputs (never
//! from the request payload, review 048/050), install it on a [`PolicyEngine`]
//! exactly as a live command would, optionally pre-spend the host-call budget
//! (`prefill_calls`), then assert [`PolicyEngine::check`] produces the expected
//! decision. This is the executable contract for `spec/policy-gates.md`.

use forge_domain::{ActorContext, Capabilities, Limits, Manifest, Role};
use forge_policy::{
    Category, ComposedDecisionContext, HostCall, PlatformPermissions, PolicyEngine, RunProfile,
    WorkspacePolicy,
};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/policy
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/policy-gates")
        .canonicalize()
        .expect("policy-gates fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

/// Parse a JSON array of category names (`["storage","db",...]`) into `Category`.
fn categories(value: &serde_json::Value) -> Vec<Category> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| serde_json::from_value(c.clone()).expect("category name deserializes"))
                .collect()
        })
        .unwrap_or_default()
}

/// Build the live `ComposedDecisionContext` from a case's TRUSTED gate inputs.
fn composed_context(case: &serde_json::Value) -> ComposedDecisionContext {
    let wp = &case["workspace_policy"];
    let workspace =
        WorkspacePolicy::new(categories(&wp["allowed"]), categories(&wp["denied"]));

    let rp = &case["run_profile"];
    let run_profile = RunProfile::new(
        rp["name"].as_str().unwrap_or("default").to_string(),
        categories(&rp["permitted"]),
    );

    let platform = PlatformPermissions::new(categories(&case["platform"]["granted"]));

    ComposedDecisionContext::new(workspace, run_profile, platform)
}

/// Build the trusted `Manifest` from a case's `manifest` (capabilities +
/// max_host_calls). Limits start from the conservative defaults and only
/// `max_host_calls` is overridden, so a fixture need not spell out every limit.
fn manifest(case: &serde_json::Value) -> Manifest {
    let m = &case["manifest"];
    let capabilities: Capabilities = serde_json::from_value(m["capabilities"].clone())
        .expect("manifest.capabilities deserializes into Capabilities");
    let max_host_calls = m["limits"]["max_host_calls"]
        .as_u64()
        .expect("manifest.limits.max_host_calls is set");
    Manifest {
        entrypoint: "src/main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities,
        limits: Limits { max_host_calls, ..Limits::default() },
    }
}

fn actor(case: &serde_json::Value) -> ActorContext {
    let role: Role = serde_json::from_value(case["actor"]["role"].clone())
        .expect("actor.role deserializes into Role");
    ActorContext { actor: "vector-actor".into(), role }
}

fn request(case: &serde_json::Value) -> HostCall {
    serde_json::from_value(case["request"].clone())
        .expect("request deserializes into HostCall")
}

/// Run one vector: build the engine with the composed trusted context, spend any
/// prefill budget, then check the asserted request and compare to `expect`.
fn run_vector(name: &str, case: &serde_json::Value) {
    let mut engine =
        PolicyEngine::with_context(&manifest(case), &actor(case), Box::new(composed_context(case)))
            .unwrap_or_else(|e| panic!("case {name}: engine build failed: {e}"));

    // Some cases pre-spend the host-call budget so the asserted call is the one
    // that trips the rate/resource-limit gate. The prefill call is the same
    // shape as the asserted request and is expected to be allowed.
    if let Some(n) = case.get("prefill_calls").and_then(|v| v.as_u64()) {
        let prefill = request(case);
        for i in 0..n {
            engine
                .check(&prefill)
                .unwrap_or_else(|e| panic!("case {name}: prefill call {i} should be allowed: {e}"));
        }
    }

    let expect = &case["expect"];
    let decision = expect["decision"].as_str().expect("expect.decision");
    let result = engine.check(&request(case));

    match decision {
        "allow" => assert!(
            result.is_ok(),
            "case {name}: expected ALLOW but got {:?}",
            result.unwrap_err()
        ),
        "deny" => {
            let err = result
                .err()
                .unwrap_or_else(|| panic!("case {name}: expected DENY but the call was allowed"));
            if let Some(code) = expect["code"].as_str() {
                assert_eq!(
                    err.code(),
                    code,
                    "case {name}: expected error code {code}, got {err}"
                );
            }
            if let Some(reason) = expect["reason_contains"].as_str() {
                if !reason.is_empty() {
                    assert!(
                        err.to_string().contains(reason),
                        "case {name}: error {err:?} must contain reason {reason:?}"
                    );
                }
            }
        }
        other => panic!("case {name}: unknown decision {other:?}"),
    }
}

fn suite() -> serde_json::Value {
    load("manifest.json")
}

#[test]
fn every_listed_case_matches_its_expected_gate_decision() {
    let suite = suite();
    let cases = suite["cases"].as_array().expect("cases is an array");
    let mut checked = 0usize;
    for entry in cases {
        let name = entry["case"].as_str().expect("case name");
        let file = entry["file"].as_str().expect("file name");
        let case = load(file);
        run_vector(name, &case);
        checked += 1;
    }
    assert_eq!(checked, 12, "expected 12 policy-gate vectors, checked {checked}");
}

#[test]
fn suite_manifest_is_consistent_with_case_files() {
    let suite = suite();
    assert_eq!(suite["count"].as_u64(), Some(12), "manifest count is 12");
    let cases = suite["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12, "12 case entries");
    for entry in cases {
        let file = entry["file"].as_str().unwrap();
        let path = fixtures_dir().join(file);
        assert!(path.exists(), "fixture file missing: {}", path.display());
        // The per-case file's declared `gate`/`decision` must agree with the
        // suite manifest so the two never drift.
        let case = load(file);
        let manifest_gate = entry["gate"].as_str().unwrap();
        let case_gate = case["expect"]["gate"].as_str().unwrap();
        assert_eq!(
            manifest_gate, case_gate,
            "gate mismatch for {file}: manifest {manifest_gate:?} vs case {case_gate:?}"
        );
        let manifest_expect = entry["expect"].as_str().unwrap();
        let case_decision = case["expect"]["decision"].as_str().unwrap();
        assert_eq!(
            manifest_expect, case_decision,
            "decision mismatch for {file}: manifest {manifest_expect:?} vs case {case_decision:?}"
        );
    }
}
