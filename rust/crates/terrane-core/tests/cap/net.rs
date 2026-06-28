//! Engine tests for the `net` capability — the recorded-effect mechanism.

use tempfile::tempdir;
use terrane_core::cap::net::fetched_event;
use terrane_core::{fold_records_in_memory, Core, State};
use terrane_domain::Error;

use crate::helpers::req;

#[test]
fn fetched_event_folds_recorded_response_without_network() {
    let mut state = State::default();
    let records = vec![fetched_event(
        "notes",
        "http://127.0.0.1/data",
        200,
        "local response".to_string(),
    )
    .unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let resp = &state.net.fetches["notes"]["http://127.0.0.1/data"];
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "local response");
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
