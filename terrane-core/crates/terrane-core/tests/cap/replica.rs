//! Engine tests for the `replica` capability — a home's stable, minted identity.

use tempfile::tempdir;
use terrane_core::Core;

use crate::helpers::{req, FakeEdge};

#[test]
fn init_mints_a_stable_id_once_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, FakeEdge).unwrap();
    assert_eq!(core.state().replica.peer, None);

    core.dispatch(req("replica.init", &[])).unwrap();
    let peer = core.state().replica.peer.expect("identity minted");

    // Idempotent: a second init is a no-op and keeps the same id.
    let records = core.dispatch(req("replica.init", &[])).unwrap();
    assert!(records.is_empty());
    assert_eq!(core.state().replica.peer, Some(peer));

    // Replay reads the id back from the log — never re-mints it.
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().replica.peer, Some(peer));
}

#[test]
fn two_homes_mint_distinct_ids() {
    let dir = tempdir().unwrap();
    let mut a = Core::open_with(dir.path().join("a.bin"), FakeEdge).unwrap();
    let mut b = Core::open_with(dir.path().join("b.bin"), FakeEdge).unwrap();
    a.dispatch(req("replica.init", &[])).unwrap();
    b.dispatch(req("replica.init", &[])).unwrap();
    assert_ne!(
        a.state().replica.peer,
        b.state().replica.peer,
        "distinct replicas must mint distinct peers"
    );
}
