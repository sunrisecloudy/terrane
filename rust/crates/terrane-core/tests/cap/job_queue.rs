use terrane_core::{QueryValue, Request};

use crate::helpers::req;

#[test]
fn job_resource_surface_is_registered() {
    let surface = terrane_core::declared_resource_surface();
    for method in [
        "ctx.resource.job.submit",
        "ctx.resource.job.cancel",
        "ctx.resource.job.progress",
        "ctx.resource.job.stat",
        "ctx.resource.job.list",
    ] {
        assert!(surface.contains(method), "missing {method}");
    }
}

#[test]
fn job_lifecycle_requires_host_for_edge_facts_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = terrane_core::Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["ops", "Ops"])).unwrap();
    core.dispatch(req(
        "job.submit",
        &["ops", "job-1", "work", r#"["payload"]"#, "", "1000"],
    ))
    .unwrap();

    let public_err = core
        .dispatch(Request::new(
            "job.start",
            vec![
                "ops".into(),
                "job-1".into(),
                "1".into(),
                "1001".into(),
                "61001".into(),
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
        "job.start",
        vec![
            "ops".into(),
            "job-1".into(),
            "1".into(),
            "1001".into(),
            "61001".into(),
        ],
    ))
    .unwrap();
    core.dispatch(req(
        "job.progress",
        &["ops", "job-1", "1", "25", "warming", "2000", "62000"],
    ))
    .unwrap();
    core.dispatch(Request::trusted_host(
        "job.report",
        vec![
            "ops".into(),
            "job-1".into(),
            "1".into(),
            "completed".into(),
            "ok".into(),
            "3000".into(),
            String::new(),
        ],
    ))
    .unwrap();
    assert!(core.replay_matches().unwrap());
    assert_eq!(core.state().job.jobs["ops"]["job-1"].status.as_str(), "done");
}

#[test]
fn job_due_query_is_pure_over_folded_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = terrane_core::Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["ops", "Ops"])).unwrap();
    core.dispatch(req(
        "job.submit",
        &["ops", "job-1", "work", r#"[]"#, "", "1000"],
    ))
    .unwrap();
    let value = core
        .query("job", "due", &["1000".to_string()])
        .expect("job.due");
    let QueryValue::Json(json) = value else {
        panic!("expected json");
    };
    assert!(json.contains(r#""action":"start""#), "{json}");
    assert!(json.contains(r#""job_id":"job-1""#), "{json}");
}

#[test]
fn app_remove_clears_job_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = terrane_core::Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["ops", "Ops"])).unwrap();
    core.dispatch(req(
        "job.submit",
        &["ops", "job-1", "work", r#"[]"#, "", "1000"],
    ))
    .unwrap();
    core.dispatch(Request::trusted_host(
        "app.remove",
        vec!["ops".into()],
    ))
    .unwrap();
    assert!(!core.state().job.jobs.contains_key("ops"));
    assert!(core.replay_matches().unwrap());
}
