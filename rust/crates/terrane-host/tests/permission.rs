use std::path::{Path, PathBuf};

use tempfile::tempdir;

fn app_fixture(root: &Path, id: &str, resources: &[&str]) -> PathBuf {
    let app_dir = root.join(id);
    std::fs::create_dir_all(&app_dir).unwrap();
    let resources_json = resources
        .iter()
        .map(|resource| format!(r#""{resource}""#))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        app_dir.join("manifest.json"),
        format!(
            r#"{{
  "id":"{id}",
  "name":"{id}",
  "version":"0.1.0",
  "runtime":"js",
  "backend":"main.js",
  "resources":[{resources_json}]
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        app_dir.join("main.js"),
        "export default async function handle() { return ''; }\n",
    )
    .unwrap();
    app_dir
}

fn install(core: &mut terrane_host::HostCore, id: &str, source: &Path) {
    terrane_host::dispatch_on_core(
        core,
        "app.add",
        &[
            id.to_string(),
            id.to_string(),
            "--source".to_string(),
            source.to_str().unwrap().to_string(),
        ],
    )
    .unwrap();
}

fn grant(core: &mut terrane_host::HostCore, app: &str, namespace: &str) {
    terrane_host::dispatch_on_core(
        core,
        "auth.grant",
        &[
            terrane_host::LOCAL_OWNER_SUBJECT.to_string(),
            app.to_string(),
            namespace.to_string(),
        ],
    )
    .unwrap();
}

#[test]
fn permission_request_ids_distinguish_lossy_tokens() {
    let missing = vec!["kv".to_string()];
    let colon =
        terrane_host::permission::permission_request_id("crm:app", "user:local-owner", &missing);
    let slash =
        terrane_host::permission::permission_request_id("crm/app", "user:local-owner", &missing);

    assert_ne!(colon, slash);
}

#[test]
fn permission_required_reports_only_grantable_missing_resources() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let source = app_fixture(dir.path(), "demo", &["kv", "net", "relational_db"]);
    let mut core = terrane_host::open_at_home(&home).unwrap();
    install(&mut core, "demo", &source);
    grant(&mut core, "demo", "kv");

    let required = terrane_host::permission::permission_required_for_app_with_admin_base(
        &core,
        "demo",
        "http://127.0.0.1:49152/",
    )
    .unwrap()
    .unwrap();

    assert_eq!(required.missing_resources, vec!["relational_db"]);
    assert!(required
        .admin_url
        .starts_with("http://127.0.0.1:49152/__terrane/admin/requests/"));
}

#[test]
fn permission_required_is_none_when_requested_resources_are_granted() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let source = app_fixture(dir.path(), "demo", &["kv", "crdt"]);
    let mut core = terrane_host::open_at_home(&home).unwrap();
    install(&mut core, "demo", &source);
    grant(&mut core, "demo", "kv");
    grant(&mut core, "demo", "crdt");

    assert!(
        terrane_host::permission::permission_required_for_app_with_admin_base(
            &core,
            "demo",
            "http://127.0.0.1:49152",
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn request_permission_persists_pending_and_approve_grants() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let source = app_fixture(dir.path(), "demo", &["kv"]);
    let mut core = terrane_host::open_at_home(&home).unwrap();
    install(&mut core, "demo", &source);

    let required = terrane_host::permission::request_permission_for_app_with_admin_base(
        &mut core,
        "demo",
        "list",
        "mcp",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .expect("missing kv should create a request");
    assert_eq!(required.request_status, "pending");

    let listed =
        terrane_host::permission::permission_requests(&core, "http://127.0.0.1:49152").unwrap();
    assert_eq!(listed.requests.len(), 1);
    assert_eq!(listed.requests[0].request_id, required.request_id);
    assert_eq!(listed.requests[0].status, "pending");

    let approved = terrane_host::permission::approve_permission_request(
        &mut core,
        &required.request_id,
        "ok",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .unwrap();
    assert_eq!(approved.status, "approved");
    assert!(
        terrane_host::permission::permission_required_for_app(&core, "demo")
            .unwrap()
            .is_none(),
        "approval should emit normal auth.granted facts"
    );
}

#[test]
fn deny_and_cancel_keep_permission_required() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let source = app_fixture(dir.path(), "demo", &["kv"]);
    let mut core = terrane_host::open_at_home(&home).unwrap();
    install(&mut core, "demo", &source);

    let required = terrane_host::permission::request_permission_for_app_with_admin_base(
        &mut core,
        "demo",
        "list",
        "mcp",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .unwrap();
    let denied = terrane_host::permission::deny_permission_request(
        &mut core,
        &required.request_id,
        "no",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .unwrap();
    assert_eq!(denied.status, "denied");
    assert!(
        terrane_host::permission::permission_required_for_app(&core, "demo")
            .unwrap()
            .is_some()
    );

    let source = app_fixture(dir.path(), "demo2", &["kv"]);
    install(&mut core, "demo2", &source);
    let required = terrane_host::permission::request_permission_for_app_with_admin_base(
        &mut core,
        "demo2",
        "list",
        "mcp",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .unwrap();
    let cancelled = terrane_host::permission::cancel_permission_request(
        &mut core,
        &required.request_id,
        "stale",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(
        terrane_host::permission::permission_required_for_app(&core, "demo2")
            .unwrap()
            .is_some()
    );
}
