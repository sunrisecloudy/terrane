//! Durability of the event log: in-process opens share one home lock (so
//! seed-then-verify and replay patterns never self-block), and multi-command
//! histories frame and replay identically after the single-buffer append change.
//! Cross-*process* rejection is proven at the host level (`terrane-host` spawns a
//! real second binary). Tests live in their own file.

use tempfile::tempdir;
use terrane_core::{read_log, Core, Request};

#[test]
fn in_process_opens_share_the_home_lock() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    // Two live Cores on one home in this process both succeed — the registry
    // shares a single OS lock. Only a different *process* is rejected.
    let a = Core::open(&log).unwrap();
    let b = Core::open(&log).unwrap();
    drop(a);
    drop(b);

    // Once every handle drops, the lock is free and a fresh open still works.
    let _c = Core::open(&log).unwrap();
}

#[test]
fn multi_command_history_frames_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(Request::new(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();
    core.dispatch(Request::new(
        "kv.set",
        vec!["notes".into(), "a".into(), "1".into()],
    ))
    .unwrap();
    core.dispatch(Request::new(
        "kv.set",
        vec!["notes".into(), "b".into(), "2".into()],
    ))
    .unwrap();
    // Removing the app cascades through broadcast fold — another commit.
    core.dispatch(Request::new("app.remove", vec!["notes".into()]))
        .unwrap();

    // Every commit round-trips: the frame lengths are intact (read_log parses the
    // full history) and replay reproduces the live state exactly.
    let records = read_log(&log).unwrap();
    assert!(
        records.len() >= 4,
        "expected at least one record per commit, got {}",
        records.len()
    );
    assert!(
        core.replay_matches().unwrap(),
        "log must replay to identical state"
    );

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().kv.data, core.state().kv.data);
    assert!(reopened.state().kv.data.is_empty());
}
