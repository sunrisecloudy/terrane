//! Engine tests for the `replica` capability — a home's stable, minted identity.

use tempfile::tempdir;
use terrane_core::cap::replica::initialized_event;
use terrane_core::{fold_records_in_memory, Core, State};

use crate::helpers::req;

#[test]
fn initialized_event_folds_stable_id() {
    let mut state = State::default();

    fold_records_in_memory(&mut state, &[initialized_event(42).unwrap()]).unwrap();

    assert_eq!(state.replica.peer, Some(42));
}

#[test]
fn init_requires_runner_when_peer_is_missing() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert_eq!(core.state().replica.peer, None);

    assert!(core
        .dispatch(req("replica.init", &[]))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}
