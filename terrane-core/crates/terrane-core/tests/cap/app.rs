//! Engine tests for the `app` capability (and core dispatch/routing).

use tempfile::tempdir;
use terrane_core::Core;
use terrane_domain::Error;

use crate::helpers::req;

#[test]
fn dispatches_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("app.add", &["tasks", "Task", "Workbench"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();

    assert!(core.replay_matches().unwrap());
    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.state().app.apps.contains_key("tasks"));

    // A brand-new Core opened on the same log rebuilds the same world.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn source_round_trips_through_the_log() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes", "--source", "apps/notes"]))
        .unwrap();
    let reopened = Core::open(&log).unwrap();
    assert_eq!(
        reopened.state().app.apps["notes"].source.as_deref(),
        Some("apps/notes")
    );
}

#[test]
fn rejects_duplicate_missing_and_unknown() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert_eq!(
        core.dispatch(req("app.add", &["notes", "Again"])),
        Err(Error::AppExists("notes".into()))
    );
    assert_eq!(
        core.dispatch(req("app.remove", &["ghost"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    // Unknown namespace and unknown verb are both rejected.
    assert!(matches!(
        core.dispatch(req("bogus.thing", &[])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.frobnicate", &["x"])),
        Err(Error::InvalidInput(_))
    ));

    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn rejects_empty_fields() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(matches!(
        core.dispatch(req("app.add", &["", "x"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.add", &["x"])),
        Err(Error::InvalidInput(_))
    ));
}
