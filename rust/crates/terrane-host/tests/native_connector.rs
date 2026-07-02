use tempfile::tempdir;
use terrane_core::QueryValue;
use terrane_host::native::{
    drain_once_on_core, observe_connector_on_core, pending_requests_for_connector, NativeConnector,
    NativeConnectorInfo, NativeDrainOutcome, NativeExecutionResult,
};

#[derive(Clone)]
struct FakeConnector {
    info: NativeConnectorInfo,
}

impl FakeConnector {
    fn new() -> Self {
        Self {
            info: NativeConnectorInfo::new(
                "fake-native-host",
                "macos",
                "test-1",
                ["external.openUrl".to_string()],
            ),
        }
    }
}

impl NativeConnector for FakeConnector {
    fn info(&self) -> NativeConnectorInfo {
        self.info.clone()
    }

    fn execute(&self, request: &terrane_cap_native::NativeRequestRecord) -> NativeExecutionResult {
        assert_eq!(request.operation_id, "external.openUrl");
        NativeExecutionResult::completed_json(serde_json::json!({
            "opened": true,
            "requestId": request.request_id,
        }))
    }
}

#[test]
fn fake_connector_observes_support_and_drains_one_pending_request() {
    let dir = tempdir().unwrap();
    let mut core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();
    terrane_host::dispatch_on_core(&mut core, "app.add", &["demo".into(), "Demo".into()]).unwrap();

    let connector = FakeConnector::new();
    let observed = observe_connector_on_core(&mut core, &connector).unwrap();
    assert_eq!(observed.records[0].kind, "native.platform.observed");
    assert_eq!(
        terrane_host::query_on_core(&core, "native", "supports", &["external.openUrl".into()])
            .unwrap(),
        QueryValue::Bool(true)
    );

    terrane_host::dispatch_on_core(
        &mut core,
        "native.external.open-url",
        &["demo".into(), "req-1".into(), "https://example.com".into()],
    )
    .unwrap();
    assert_eq!(pending_requests_for_connector(&core, &connector).len(), 1);

    let drained = drain_once_on_core(&mut core, &connector).unwrap();
    let NativeDrainOutcome::Drained(drained) = drained else {
        panic!("expected one request to drain")
    };
    assert_eq!(drained.app, "demo");
    assert_eq!(drained.request_id, "req-1");
    assert_eq!(drained.operation_id, "external.openUrl");
    assert_eq!(drained.records[0].kind, "native.completed");
    assert_eq!(
        core.state().native.requests["demo"]["req-1"]
            .result_json
            .as_deref(),
        Some(r#"{"opened":true,"requestId":"req-1"}"#)
    );
    assert_eq!(
        drain_once_on_core(&mut core, &connector).unwrap(),
        NativeDrainOutcome::Idle
    );
}
