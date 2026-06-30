//! Broker core-logic tests: `permission_required_for_app` computes the missing
//! grant set and goes empty once granted. Complements `tests/permission.rs`
//! (which covers request-id distinctness). Tests live in their own file.

use std::fs;

use tempfile::tempdir;
use terrane_host::permission::permission_required_for_app;
use terrane_host::{dispatch_on_core, open_at_log_path, LOCAL_OWNER_SUBJECT};

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// Install a `demo` app whose manifest requests `kv`, into a throwaway home.
fn install_kv_app(dir: &std::path::Path) -> terrane_host::HostCore {
    let bundle = dir.join("demo");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"demo","name":"Demo","runtime":"js","backend":"main.js","resources":["kv"]}"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), "export function handle() { return ''; }").unwrap();
    let src = bundle.to_str().unwrap().to_string();

    let mut core = open_at_log_path(dir.join("log.bin")).unwrap();
    dispatch_on_core(&mut core, "app.add", &s(&["demo", "Demo", "--source", &src])).unwrap();
    core
}

#[test]
fn broker_reports_missing_grant_then_none_after_grant() {
    let dir = tempdir().unwrap();
    let mut core = install_kv_app(dir.path());

    // Before any grant: kv is requested but not granted.
    let required = permission_required_for_app(&core, "demo")
        .unwrap()
        .expect("kv grant should be required before granting");
    assert_eq!(required.missing_resources, vec!["kv".to_string()]);
    assert!(
        required
            .grant_commands
            .iter()
            .any(|c| c.contains("auth grant") && c.contains("kv")),
        "expected a copy-paste grant command, got {:?}",
        required.grant_commands
    );

    // After granting kv: nothing is required.
    dispatch_on_core(&mut core, "auth.grant", &s(&[LOCAL_OWNER_SUBJECT, "demo", "kv"])).unwrap();
    assert!(
        permission_required_for_app(&core, "demo").unwrap().is_none(),
        "no permission should be required once kv is granted"
    );
}
