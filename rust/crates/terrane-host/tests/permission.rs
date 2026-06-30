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
    let colon = terrane_host::permission::permission_request_id(
        "crm:app",
        "user:local-owner",
        &missing,
    );
    let slash = terrane_host::permission::permission_request_id(
        "crm/app",
        "user:local-owner",
        &missing,
    );

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
