//! Integration tests for Phase C bridge/package commands.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, CoreCommand, RequestId, WorkspaceId};

fn host_cmd(name: &str, payload: serde_json::Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("bridge-security-test"),
        actor: ActorContext::owner("macos-host"),
        workspace_id: WorkspaceId::new("macos-native"),
        applet_id: None,
        name: name.into(),
        payload,
    }
}

#[test]
fn bridge_validate_network_request_command_roundtrip() {
    let mut core = WorkspaceCore::in_memory("macos-native").unwrap();
    let payload = serde_json::json!({
        "network_policy": {
            "allow": [{ "origin": "https://api.example.com", "methods": ["GET"] }]
        },
        "request": { "url": "https://api.example.com/x", "method": "GET" }
    });
    let out = core.handle(host_cmd("bridge.validate_network_request", payload));
    assert!(out.ok, "{:?}", out.error);
    assert_eq!(out.payload["allowed"], serde_json::json!(true));
}

#[test]
fn package_get_permissions_from_trusted_manifest() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../webapps/examples/api-dashboard/manifest.json"
    ))
    .unwrap();
    let mut core = WorkspaceCore::in_memory("macos-native").unwrap();
    let out = core.handle(host_cmd(
        "package.get_permissions",
        serde_json::json!({
            "app_id": "api-dashboard",
            "manifest_json": manifest,
        }),
    ));
    assert!(out.ok, "{:?}", out.error);
    assert!(out.payload["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "network.request"));
}

#[test]
fn bridge_record_call_emits_deterministic_id() {
    let mut core = WorkspaceCore::in_memory("macos-native").unwrap();
    let out = core.handle(host_cmd(
        "bridge.record_call",
        serde_json::json!({
            "record": {
                "platform_ids": { "platform": "macos", "target": "native" },
                "session_id": "runtime_macos_native_notes-lite_mount",
                "request_id": "req1",
                "app_id": "notes-lite",
                "method": "storage.get",
                "params": { "key": "notes-lite:x" },
                "ok": true,
                "result": { "value": null }
            }
        }),
    ));
    assert!(out.ok, "{:?}", out.error);
    assert_eq!(out.payload["bridge_call_id"], "bridge_macos_native_req1");
}