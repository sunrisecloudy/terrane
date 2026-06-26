//! Engine tests for the `net` capability — the recorded-effect mechanism.

use tempfile::tempdir;
use terrane_core::Core;
use terrane_domain::Error;

use crate::helpers::{req, FakeEdge};

#[test]
fn fetch_effect_is_recorded_then_replays_without_the_runner() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open_with(&log, FakeEdge).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("net.fetch", &["notes", "http://example.test/data"]))
        .unwrap();

    let resp = &core.state().net.fetches["notes"]["http://example.test/data"];
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "body for http://example.test/data");

    let records = terrane_core::read_log(&log).unwrap();
    assert!(records.iter().any(|r| r.kind == "net.fetched"));

    // Reopening with NO runner folds the log and reproduces the fetch — proof
    // that replay reads the body from the log, not the network.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().net.fetches, core.state().net.fetches);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn fetch_is_validated_purely_before_any_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    // A pure core (NoEffects): a valid Fetch reaches the runner and is refused…
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert!(matches!(
        core.dispatch(req("net.fetch", &["notes", "http://x/"])),
        Err(Error::InvalidInput(_))
    ));
    // …but a Fetch for a missing app is rejected in decide, before the runner.
    assert_eq!(
        core.dispatch(req("net.fetch", &["ghost", "http://x/"])),
        Err(Error::AppNotFound("ghost".into()))
    );
}
