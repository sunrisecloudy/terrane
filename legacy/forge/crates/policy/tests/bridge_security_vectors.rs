//! Data-driven Phase C bridge-security vectors (`forge/fixtures/bridge-security/`).

use forge_policy::{
    bridge_call_id, core_event_id, runtime_session_id, validate_bridge_envelope,
    BridgeEnvelopeRequest, BridgePlatformIds, WebappNetRequest, check_webapp_network,
};
use forge_domain::WebappNetworkPolicy;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bridge-security")
        .canonicalize()
        .expect("bridge-security fixtures dir exists")
}

fn load(name: &str) -> serde_json::Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

#[test]
fn bridge_security_vectors_match_manifest() {
    let manifest = load("manifest.json");
    for case in manifest["cases"].as_array().expect("cases array") {
        let name = case.as_str().expect("case name");
        let vector = load(&format!("{name}.json"));
        match vector["kind"].as_str().expect("kind") {
            "network" => {
                let policy: WebappNetworkPolicy =
                    serde_json::from_value(vector["network_policy"].clone()).unwrap();
                let request: WebappNetRequest =
                    serde_json::from_value(vector["request"].clone()).unwrap();
                let decision = check_webapp_network(&policy, &request, None);
                match vector["expect"].as_str().unwrap() {
                    "allow" => assert!(decision.allowed, "case {name} should allow: {decision:?}"),
                    "deny" => assert!(!decision.allowed, "case {name} should deny"),
                    other => panic!("case {name}: unknown expect {other}"),
                }
            }
            "envelope" => {
                let input: BridgeEnvelopeRequest =
                    serde_json::from_value(vector["input"].clone()).unwrap();
                let decision = validate_bridge_envelope(&input);
                match vector["expect"].as_str().unwrap() {
                    "allow" => assert!(decision.allowed, "case {name} should allow"),
                    "deny" => {
                        assert!(!decision.allowed, "case {name} should deny");
                        if let Some(code) = vector.get("error_code").and_then(|v| v.as_str()) {
                            assert_eq!(decision.error_code.as_deref(), Some(code), "case {name}");
                        }
                        if vector.get("quarantine_eligible").and_then(|v| v.as_bool()) == Some(true) {
                            assert!(decision.quarantine_eligible, "case {name}");
                        }
                    }
                    other => panic!("case {name}: unknown expect {other}"),
                }
            }
            "record" => {
                let ids: BridgePlatformIds =
                    serde_json::from_value(vector["platform_ids"].clone()).unwrap();
                let request_id = vector["request_id"].as_str().unwrap();
                let app_id = vector["app_id"].as_str().unwrap();
                let mount = vector["mount_token"].as_str().unwrap();
                let expect = &vector["expect"];
                assert_eq!(
                    bridge_call_id(&ids, request_id),
                    expect["bridge_call_id"].as_str().unwrap()
                );
                assert_eq!(
                    core_event_id(&ids, request_id),
                    expect["core_event_id"].as_str().unwrap()
                );
                assert_eq!(
                    runtime_session_id(&ids, app_id, mount),
                    expect["session_id"].as_str().unwrap()
                );
            }
            other => panic!("case {name}: unknown kind {other}"),
        }
    }
}