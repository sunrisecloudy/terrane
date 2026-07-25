use tempfile::tempdir;
use terrane_cap_interface::Request;
use terrane_core::Core;

#[test]
fn legacy_app_requirements_are_resolved_once_and_replay() {
    let home = tempdir().unwrap();
    let log = home.path().join("log.bin");
    {
        let mut core = Core::open(&log).unwrap();
        core.dispatch(Request::new(
            "app.add",
            vec!["legacy".into(), "Legacy".into()],
        ))
        .unwrap();
        assert!(!core.state().app.apps["legacy"].requirements_resolved);
    }

    let first_count;
    {
        let core = terrane_host::open_at_home(home.path()).unwrap();
        let app = &core.state().app.apps["legacy"];
        assert!(app.requirements_resolved);
        assert!(app.required_capabilities.is_empty());
        first_count = core.log_records().unwrap().len();
        assert!(core
            .log_records()
            .unwrap()
            .iter()
            .any(|record| record.kind == "app.requirements.resolved"));
    }

    let core = terrane_host::open_at_home(home.path()).unwrap();
    assert_eq!(core.log_records().unwrap().len(), first_count);
    assert!(core.replay_matches().unwrap());
}
