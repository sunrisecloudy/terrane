//! `ctx.resource` capability vectors (camera first, handle-based assets).

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use forge_domain::{ActorContext, Capabilities, FilesGrant, FileRule, Limits, Manifest};
use forge_runtime::{
    HostContext, InMemoryFileSystem, MemoryHostBridge, ResourceMaterializeRequest, RunRecorder,
};
use forge_policy::{Category, ComposedDecisionContext, HostCall, PlatformPermissions, PolicyEngine};

fn camera_manifest(files: FilesGrant) -> Manifest {
    Manifest {
        entrypoint: "src/main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities: Capabilities {
            ui: true,
            resources: vec!["camera".into()],
            files,
            ..Capabilities::default()
        },
        limits: Limits {
            max_host_calls: 100,
            ..Limits::default()
        },
        compatibility: Default::default(),
    }
}

#[test]
fn camera_invoke_returns_metadata_not_inline_bytes() {
    let manifest = camera_manifest(FilesGrant::default());
    let actor = ActorContext::owner("tester");
    let mut bridge = MemoryHostBridge::new();
    let mut host = HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
        .unwrap();

    let shot = host.resource_invoke("camera".into(), serde_json::Value::Null).unwrap();
    assert!(shot.asset_id.starts_with("res_camera_"));
    assert_eq!(shot.content_type, "image/jpeg");
    assert!(shot.size_bytes > 0);
    assert!(shot.width.is_some());

    let assets = host.resource_assets();
    assert!(assets.contains_key(&shot.asset_id));
    assert!(!assets[&shot.asset_id].bytes_base64.is_empty());

    let (recorder, _) = host.finish();
    let calls = recorder.into_calls();
    let invoke = calls.iter().find(|c| c.method == "resource.invoke").unwrap();
    assert!(invoke.response.get("asset_id").is_some());
    assert!(invoke.response.get("bytes_base64").is_none());
}

#[test]
fn camera_invoke_without_grant_is_capability_required() {
    let manifest = Manifest {
        entrypoint: "src/main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities: Capabilities { ui: true, ..Capabilities::default() },
        limits: Limits::default(),
        compatibility: Default::default(),
    };
    let actor = ActorContext::owner("tester");
    let mut bridge = MemoryHostBridge::new();
    let mut host = HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
        .unwrap();

    let err = host
        .resource_invoke("camera".into(), serde_json::Value::Null)
        .unwrap_err();
    assert_eq!(err.code(), "CapabilityRequired");
}

#[test]
fn materialize_copies_into_files_without_js_base64() {
    let files = FilesGrant {
        write: vec![FileRule {
            handle: "workspace_data".into(),
            path_glob: "attachments/**".into(),
            max_bytes: Some(1_000_000),
            content_types: vec!["image/jpeg".into()],
        }],
        ..FilesGrant::default()
    };
    let manifest = camera_manifest(files);
    let actor = ActorContext::owner("tester");
    let fs = InMemoryFileSystem::new().with_handle_root("workspace_data", "/sandbox");
    let mut bridge = MemoryHostBridge::new().with_file_system(fs);
    let mut host = HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
        .unwrap();

    let shot = host.resource_invoke("camera".into(), serde_json::Value::Null).unwrap();
    host.resource_materialize(
        shot.asset_id.clone(),
        ResourceMaterializeRequest {
            handle: "workspace_data".into(),
            path: "attachments/photo.jpg".into(),
        },
    )
    .unwrap();

    let file = bridge
        .peek_file("workspace_data", "attachments/photo.jpg")
        .expect("materialized file exists");
    assert_eq!(file.content_type.as_deref(), Some("image/jpeg"));
    assert!(!file.bytes.is_empty());
}

#[test]
fn resource_read_returns_base64_on_second_call() {
    let manifest = camera_manifest(FilesGrant::default());
    let actor = ActorContext::owner("tester");
    let mut bridge = MemoryHostBridge::new();
    let mut host = HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
        .unwrap();

    let shot = host.resource_invoke("camera".into(), serde_json::Value::Null).unwrap();
    let read = host
        .resource_read(shot.asset_id, forge_runtime::ResourceReadRequest::default())
        .unwrap();
    assert!(!read.bytes_base64.is_empty());
    BASE64.decode(read.bytes_base64.as_bytes()).unwrap();
}

#[test]
fn platform_permission_gate_blocks_camera_when_unavailable() {
    let manifest = camera_manifest(FilesGrant::default());
    let actor = ActorContext::owner("tester");
    let context = ComposedDecisionContext::new(
        forge_policy::WorkspacePolicy::new([Category::Resource], []),
        forge_policy::RunProfile::new("default", [Category::Resource]),
        PlatformPermissions::new([]),
    );
    let mut policy = PolicyEngine::with_context(&manifest, &actor, Box::new(context)).unwrap();
    let err = policy
        .check(&HostCall::Resource {
            kind: "camera".into(),
            args: serde_json::Value::Null,
        })
        .unwrap_err();
    assert_eq!(err.code(), "PlatformUnavailable");
}