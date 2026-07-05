use terrane_core::Request;

use crate::helpers::req;

#[test]
fn scheduler_resource_surface_is_registered() {
    let surface = terrane_core::declared_resource_surface();
    for method in [
        "ctx.resource.scheduler.set",
        "ctx.resource.scheduler.clear",
        "ctx.resource.scheduler.list",
        "ctx.resource.scheduler.stat",
    ] {
        assert!(surface.contains(method), "missing {method}");
    }
}

#[test]
fn scheduler_public_state_replays_and_cleanup_on_app_remove() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = terrane_core::Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["ops", "Ops"])).unwrap();
    core.dispatch(req(
        "scheduler.set",
        &[
            "ops",
            "quickjs-ops-heartbeat",
            r#"{"at":1000,"verb":"on_timer","args":["payload"]}"#,
        ],
    ))
    .unwrap();
    let public_err = core
        .dispatch(Request::new(
            "scheduler.fire",
            vec![
                "ops".into(),
                "quickjs-ops-heartbeat".into(),
                "1000".into(),
                "1001".into(),
                "0".into(),
            ],
        ))
        .unwrap_err();
    assert!(
        public_err
            .to_string()
            .contains("requires trusted host authority"),
        "{public_err}"
    );

    core.dispatch(Request::trusted_host(
        "scheduler.fire",
        vec![
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            "1000".into(),
            "1001".into(),
            "0".into(),
        ],
    ))
    .unwrap();
    assert!(core.replay_matches().unwrap());
    assert!(!core.state().scheduler.schedules["ops"].contains_key("quickjs-ops-heartbeat"));

    core.dispatch(Request::trusted_host(
        "app.remove",
        vec!["ops".into()],
    ))
    .unwrap();
    assert!(!core.state().scheduler.schedules.contains_key("ops"));
}
