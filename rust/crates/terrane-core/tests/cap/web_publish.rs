use tempfile::tempdir;
use terrane_core::{Core, Error, QueryValue};

use crate::helpers::req;

#[test]
fn web_publish_enable_domain_status_and_replay_identity() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let records = core
        .dispatch(req(
            "web-publish.enable",
            &["demo", "interactive", "demo-live"],
        ))
        .unwrap();
    assert_eq!(records[0].kind, "web-publish.enabled");
    core.dispatch(req(
        "web-publish.domain.set",
        &["demo", "demo.example.com"],
    ))
    .unwrap();

    let QueryValue::Json(status) =
        core.query("web-publish", "status", &["demo".to_string()]).unwrap()
    else {
        panic!("web-publish.status should return JSON");
    };
    assert!(status.contains(r#""enabled":true"#), "{status}");
    assert!(status.contains(r#""mode":"interactive""#), "{status}");
    assert!(status.contains(r#""url":"https://demo.example.com""#), "{status}");

    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().web_publish,
        core.state().web_publish
    );
}

#[test]
fn web_publish_validation_and_disable_are_replay_safe() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    assert!(matches!(
        core.dispatch(req("web-publish.enable", &["demo", "bad-mode"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("web-publish.enable", &["ghost", "static"])),
        Err(Error::AppNotFound(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "web-publish.domain.set",
            &["demo", "demo.example.com"]
        )),
        Err(Error::InvalidInput(_))
    ));

    core.dispatch(req("web-publish.enable", &["demo", "static", "demo-live"]))
        .unwrap();
    core.dispatch(req("web-publish.disable", &["demo"]))
        .unwrap();
    let QueryValue::Json(status) =
        core.query("web-publish", "status", &["demo".to_string()]).unwrap()
    else {
        panic!("web-publish.status should return JSON");
    };
    assert!(status.contains(r#""enabled":false"#), "{status}");
    assert!(core.replay_matches().unwrap());
}
