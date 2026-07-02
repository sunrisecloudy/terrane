//! Engine tests for the `native` capability request queue.

use tempfile::tempdir;
use terrane_core::{Core, Error, QueryValue, ReadValue, RuntimeHost, RuntimeResourceHost};

use crate::helpers::{grant_resource, req};

#[test]
fn native_request_lifecycle_is_replay_safe() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    assert_eq!(
        core.query("native", "supports", &["external.openUrl".into()])
            .unwrap(),
        QueryValue::Bool(false)
    );

    core.dispatch(req(
        "native.platform.observe",
        &["local", "macos", "test-1", "external.openUrl"],
    ))
    .unwrap();
    assert_eq!(
        core.query("native", "native.supports", &["external.openUrl".into()])
            .unwrap(),
        QueryValue::Bool(true)
    );

    let requested = core
        .dispatch(req(
            "native.external.open-url",
            &["demo", "req-1", "https://example.com"],
        ))
        .unwrap();
    assert_eq!(requested[0].kind, "native.requested");
    let record = &core.state().native.requests["demo"]["req-1"];
    assert_eq!(record.operation_id, "external.openUrl");
    assert_eq!(record.executor_host_id, "local");
    assert_eq!(record.sequence, 1);
    assert!(record.origin_replica.is_none());

    let duplicate = core
        .dispatch(req(
            "native.external.open-url",
            &["demo", "req-1", "https://example.com"],
        ))
        .unwrap_err();
    assert!(duplicate
        .to_string()
        .contains("native request already exists"));

    core.dispatch(req("native.complete", &["demo", "req-1", r#"{"ok":true}"#]))
        .unwrap();
    let record = &core.state().native.requests["demo"]["req-1"];
    assert_eq!(record.status.as_str(), "completed");
    assert_eq!(record.result_json.as_deref(), Some(r#"{"ok":true}"#));

    let second_complete = core
        .dispatch(req("native.complete", &["demo", "req-1", r#"{"ok":true}"#]))
        .unwrap_err();
    assert!(second_complete.to_string().contains("is not pending"));
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    let replayed = &reopened.state().native.requests["demo"]["req-1"];
    assert_eq!(replayed.status.as_str(), "completed");
    assert_eq!(replayed.result_json.as_deref(), Some(r#"{"ok":true}"#));
}

#[test]
fn native_requests_require_platform_and_supported_operation() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let missing_platform = core
        .dispatch(req(
            "native.clipboard.write-text",
            &["demo", "clip-1", "hello"],
        ))
        .unwrap_err();
    assert!(missing_platform
        .to_string()
        .contains("native platform has not been observed"));

    core.dispatch(req(
        "native.platform.observe",
        &["local", "macos", "test-1", "external.openUrl"],
    ))
    .unwrap();
    assert_eq!(
        core.dispatch(req(
            "native.clipboard.write-text",
            &["demo", "clip-1", "hello"],
        )),
        Err(Error::InvalidInput(
            "native operation is not supported on this host: clipboard.writeText".into()
        ))
    );
}

#[test]
fn native_runtime_resources_record_requests_and_read_later_results() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    grant_resource(&mut core, "demo", "native");
    core.dispatch(req(
        "native.platform.observe",
        &["local", "macos", "test-1", "external.openUrl"],
    ))
    .unwrap();

    let mut host = RuntimeResourceHost::new("demo", core.state().clone());
    let methods = host.resource_methods("native").unwrap();
    assert!(methods
        .iter()
        .any(|method| method.name() == "externalOpenUrl"));

    host.write_resource(
        "native",
        "externalOpenUrl",
        &["req-1".into(), "https://example.com".into()],
    )
    .unwrap();
    assert_eq!(
        host.read_resource("native", "pending", &[]).unwrap(),
        ReadValue::StringList(vec!["req-1".into()])
    );
    let records = host.take_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "native.requested");
}

#[test]
fn app_removal_drops_native_request_state() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "native.platform.observe",
        &["local", "macos", "test-1", "external.openUrl"],
    ))
    .unwrap();
    core.dispatch(req(
        "native.external.open-url",
        &["demo", "req-1", "https://example.com"],
    ))
    .unwrap();

    core.dispatch(req("app.remove", &["demo"])).unwrap();
    assert!(!core.state().native.requests.contains_key("demo"));
    assert!(core.replay_matches().unwrap());
}
