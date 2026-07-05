use terrane_core::Request;

use crate::helpers::req;

#[test]
fn scheduler_resource_surface_is_registered() {
    let surface = terrane_core::declared_resource_surface();
    for method in [
        "ctx.resource.scheduler.create",
        "ctx.resource.scheduler.list",
        "ctx.resource.scheduler.pause",
        "ctx.resource.scheduler.resume",
        "ctx.resource.scheduler.remove",
        "ctx.resource.scheduler.history",
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
        "scheduler.create",
        &[
            "ops",
            "quickjs-ops-heartbeat",
            "* * * * *",
            "Asia/Bangkok",
            "opsHeartbeat",
            r#"{"source":"premium-ops-proof"}"#,
        ],
    ))
    .unwrap();
    core.dispatch(Request::trusted_host(
        "scheduler.run.start",
        vec![
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            "run-1".into(),
            "60".into(),
        ],
    ))
    .unwrap();
    core.dispatch(Request::trusted_host(
        "scheduler.run.complete",
        vec![
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            "run-1".into(),
            "61".into(),
            r#"{"ok":true}"#.into(),
        ],
    ))
    .unwrap();
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        core.state().scheduler.runs["ops"]["run-1"].status.as_str(),
        "completed"
    );
}
