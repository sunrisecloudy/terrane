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
fn open_at_home_seeds_local_owner_membership_once() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let log = home.join("log.bin");

    let core = terrane_host::open_at_home(&home).unwrap();
    let members = terrane_cap_auth::auth_members(core.state()).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].subject, terrane_host::LOCAL_OWNER_SUBJECT);
    assert_eq!(members[0].role, "owner");

    let reopened = terrane_host::open_at_home(&home).unwrap();
    assert_eq!(
        terrane_core::read_log(&log)
            .unwrap()
            .iter()
            .filter(|record| record.kind == "auth.member.added")
            .count(),
        1,
        "local owner membership seed should be recorded exactly once"
    );
    assert_eq!(
        terrane_cap_auth::auth_members(reopened.state())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn permission_required_reports_only_grantable_missing_resources() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    // `model` is requested but not a grantable resource (no grant spec), so it
    // must be dropped from the prompt; only the grantable-but-missing
    // `relational_db` should be reported. (`net` used to be the non-grantable
    // example here, but it now exposes the grantable `net.get` resource.)
    let source = app_fixture(dir.path(), "demo", &["kv", "model", "relational_db"]);
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
    assert_eq!(required.app_name, "demo");
    assert_eq!(required.source, "mcp");
    assert!(!required.resume_token_hash.is_empty());

    let listed =
        terrane_host::permission::permission_requests(&core, "http://127.0.0.1:49152").unwrap();
    assert_eq!(listed.requests.len(), 1);
    assert_eq!(listed.requests[0].request_id, required.request_id);
    assert_eq!(listed.requests[0].status, "pending");
    assert_eq!(listed.requests[0].app_name, "demo");
    assert_eq!(listed.requests[0].source, "mcp");
    assert_eq!(
        listed.requests[0].resume_token_hash,
        required.resume_token_hash
    );

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
fn namespace_permission_preview_is_side_effect_free() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let mut core = terrane_host::open_at_home(&home).unwrap();
    terrane_host::dispatch_on_core(&mut core, "app.add", &["demo".into(), "demo".into()]).unwrap();

    let required =
        terrane_host::permission::preview_permission_required_for_namespace_with_admin_base(
            &core,
            "demo",
            "kv",
            "capability_command:kv.set",
            "mcp_stdio",
            "http://127.0.0.1:49152",
        )
        .unwrap()
        .expect("missing kv should create a preview requirement");

    assert_eq!(required.operation, "capability_command:kv.set");
    assert_eq!(required.source, "mcp_stdio");
    assert_eq!(required.missing_resources, vec!["kv"]);
    assert_eq!(required.request_status, "preview");
    assert_eq!(required.resume_tool, "");
    assert!(required.message.contains("rerun without dryRun"));
    assert!(
        terrane_cap_auth::permission_request(core.state(), &required.request_id)
            .unwrap()
            .is_none(),
        "preview must not record a pending request"
    );
}

#[test]
fn namespace_permission_request_records_direct_operation() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let mut core = terrane_host::open_at_home(&home).unwrap();
    terrane_host::dispatch_on_core(&mut core, "app.add", &["demo".into(), "demo".into()]).unwrap();

    let required = terrane_host::permission::request_permission_for_namespace_with_admin_base(
        &mut core,
        "demo",
        "kv",
        "capability_command:kv.set",
        "mcp_http",
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .expect("missing kv should create a request");

    assert_eq!(required.operation, "capability_command:kv.set");
    assert_eq!(required.source, "mcp_http");
    assert_eq!(required.missing_resources, vec!["kv"]);
    assert_eq!(required.request_status, "pending");
    assert_eq!(required.resume_tool, "permission_check");

    let view = terrane_host::permission::permission_request_view(
        &core,
        &required.request_id,
        "http://127.0.0.1:49152",
    )
    .unwrap()
    .expect("recorded request view");
    assert_eq!(view.operation, "capability_command:kv.set");
    assert_eq!(view.source, "mcp_http");
    assert_eq!(view.resources.len(), 1);
    assert_eq!(view.resources[0].namespace, "kv");
}

#[test]
fn namespace_permission_reports_none_when_granted_and_errors_for_unknown_app() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let mut core = terrane_host::open_at_home(&home).unwrap();
    terrane_host::dispatch_on_core(&mut core, "app.add", &["demo".into(), "demo".into()]).unwrap();
    grant(&mut core, "demo", "kv");

    assert!(
        terrane_host::permission::permission_required_for_namespace_with_admin_base(
            &core,
            "demo",
            "kv",
            "capability_command:kv.set",
            "mcp_stdio",
            "http://127.0.0.1:49152",
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        terrane_host::permission::permission_required_for_namespace_with_admin_base(
            &core,
            "missing",
            "kv",
            "capability_command:kv.set",
            "mcp_stdio",
            "http://127.0.0.1:49152",
        )
        .unwrap_err(),
        "no such app: missing"
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
