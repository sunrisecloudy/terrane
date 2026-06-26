//! D12 package lifecycle + auto-quarantine conformance vectors.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, CoreCommand, RequestId, WorkspaceId};
use forge_storage::{PackageAppRecord, PackageVersionRecord, PlatformRegistry};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/package-lifecycle")
        .canonicalize()
        .expect("package-lifecycle fixtures dir exists")
}

fn owner() -> ActorContext {
    ActorContext::owner("host")
}

fn cmd(name: &str, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("pkg-lifecycle"),
        actor: owner(),
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: None,
        name: name.into(),
        payload,
    }
}

fn seed_registry() -> PlatformRegistry {
    let mut registry = PlatformRegistry::default();
    registry.apps.insert(
        "notes-lite".into(),
        PackageAppRecord {
            id: "notes-lite".into(),
            name: "Notes Lite".into(),
            status: "enabled".into(),
            active_install_id: Some("install-v2".into()),
            active_version: Some("0.2.0".into()),
            data_version: 1,
        },
    );
    for (install_id, version, status, created_at) in [
        ("install-v1", "0.1.0", "installed", "2024-01-01T00:00:00Z"),
        ("install-v2", "0.2.0", "enabled", "2024-01-02T00:00:00Z"),
    ] {
        registry.versions.insert(
            install_id.into(),
            PackageVersionRecord {
                install_id: install_id.into(),
                app_id: "notes-lite".into(),
                version: version.into(),
                runtime_version: "0.4.0".into(),
                data_version: 1,
                status: status.into(),
                created_at: created_at.into(),
                activated_at: Some(created_at.into()),
            },
        );
    }
    registry
}

#[test]
fn package_lifecycle_vectors_conformance() {
    let dir = fixtures_dir();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let declared = manifest["count"].as_u64().expect("manifest.count") as usize;
    let mut ran = 0usize;
    for entry in manifest["cases"].as_array().expect("manifest.cases") {
        let case = entry["case"].as_str().expect("case name");
        let file = entry["file"].as_str().expect("case file");
        let vector: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(file)).unwrap()).unwrap();
        drive_vector(case, &vector);
        ran += 1;
    }
    assert_eq!(ran, declared, "every declared package-lifecycle vector must run");
}

fn drive_vector(case: &str, vector: &Value) {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    core.provision_platform_registry(seed_registry()).unwrap();
    let when = &vector["when"];
    let command = when["command"].as_str().unwrap();
    let payload = when["payload"].clone();
    let resp = core.handle(cmd(command, payload));
    assert!(resp.ok, "{case} must succeed: {:?}", resp.error);
    let expect = &vector["expect"];
    match case {
        "rollback_to_prior_version" => {
            assert_eq!(resp.payload["active_install_id"], expect["active_install_id"]);
            assert_eq!(resp.payload["rolled_back_install_id"], expect["rolled_back_install_id"]);
            assert_eq!(resp.payload["active_version"], expect["active_version"]);
            for action in expect["audit_actions"].as_array().unwrap() {
                let rows = core
                    .store()
                    .query_audit(&forge_storage::AuditQuery::by_action(
                        action.as_str().unwrap(),
                    ))
                    .unwrap();
                assert!(!rows.is_empty(), "missing audit action {action}");
            }
        }
        "set_status_quarantine_restore_previous" => {
            assert_eq!(resp.payload["active_install_id"], expect["active_install_id"]);
            assert_eq!(resp.payload["rolled_back_install_id"], expect["rolled_back_install_id"]);
            for action in expect["audit_actions"].as_array().unwrap() {
                let rows = core
                    .store()
                    .query_audit(&forge_storage::AuditQuery::by_action(
                        action.as_str().unwrap(),
                    ))
                    .unwrap();
                assert!(!rows.is_empty(), "missing audit action {action}");
            }
        }
        "auto_quarantine_after_third_budget_error" => {
            assert_eq!(resp.payload["should_quarantine"], expect["should_quarantine"]);
            assert_eq!(resp.payload["quarantine_eligible"], expect["quarantine_eligible"]);
            assert_eq!(
                resp.payload["transition"]["active_install_id"],
                expect["transition"]["active_install_id"]
            );
            assert_eq!(
                resp.payload["transition"]["rolled_back_install_id"],
                expect["transition"]["rolled_back_install_id"]
            );
        }
        "list_versions_returns_active_pointer" => {
            assert_eq!(resp.payload["active_install_id"], expect["active_install_id"]);
            assert_eq!(resp.payload["active_version"], expect["active_version"]);
            assert_eq!(
                resp.payload["versions"].as_array().unwrap().len(),
                expect["version_count"].as_u64().unwrap() as usize
            );
        }
        other => panic!("unhandled package lifecycle vector {other:?}"),
    }
}