use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, CoreCommand, RequestId, Role, WorkspaceId};

fn command(name: &str, role: Role, payload: serde_json::Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req"),
        actor: ActorContext {
            actor: "test".into(),
            role,
        },
        workspace_id: WorkspaceId::new("ws"),
        applet_id: None,
        name: name.into(),
        payload,
    }
}

#[test]
fn system_describe_filters_by_role_and_tier() {
    let mut core = WorkspaceCore::in_memory("ws").unwrap();
    let response = core.handle(command(
        "system.describe",
        Role::Viewer,
        serde_json::json!({ "tier": "public" }),
    ));
    assert!(response.ok, "{:?}", response.error);
    let names: Vec<_> = response.payload["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"query.execute".to_string()));
    assert!(!names.contains(&"quota.set".to_string()));
    assert!(!names.contains(&"workspace.import".to_string()));
}

#[test]
fn system_describe_namespace_filter_narrows_results() {
    let mut core = WorkspaceCore::in_memory("ws").unwrap();
    let response = core.handle(command(
        "system.describe",
        Role::Owner,
        serde_json::json!({ "tier": "operator", "namespace": "workspace" }),
    ));
    assert!(response.ok, "{:?}", response.error);
    let names: Vec<_> = response.payload["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.iter().all(|name| name.starts_with("workspace.")));
    assert!(names.contains(&"workspace.open".to_string()));
}

#[test]
fn system_describe_catalog_version_is_stable() {
    let mut core = WorkspaceCore::in_memory("ws").unwrap();
    let first = core.handle(command("system.describe", Role::Owner, serde_json::json!({})));
    let second = core.handle(command("system.describe", Role::Owner, serde_json::json!({})));
    assert_eq!(
        first.payload["catalogVersion"],
        second.payload["catalogVersion"]
    );
}