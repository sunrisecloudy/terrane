//! Engine tests for the `push` capability.

use tempfile::tempdir;
use terrane_core::{Core, ReadValue, RuntimeHost, RuntimeResourceHost};

use crate::helpers::{grant_resource, req};

#[test]
fn push_subscribe_unsubscribe_and_replay_identity() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "push.subscribe",
        &["demo", "kv.*", "Changed {key}|{value}", "sub-1"],
    ))
    .unwrap();
    assert!(core.state().push.subscriptions["demo"].contains_key("sub-1"));

    core.dispatch(req(
        "push.record-delivery",
        &["demo", "sub-1", "7", "delivered"],
    ))
    .unwrap();
    assert!(core.state().push.deliveries["demo"]["sub-1"].contains_key(&7));

    core.dispatch(req("push.unsubscribe", &["demo", "sub-1"]))
        .unwrap();
    assert!(!core.state().push.subscriptions.contains_key("demo"));
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().push, core.state().push);
}

#[test]
fn push_limits_and_typed_errors_are_enforced() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let bad = core
        .dispatch(req("push.subscribe", &["demo", "KV.*", "Title"]))
        .unwrap_err();
    assert!(bad.to_string().contains("event_pattern must be"));

    for i in 0..terrane_cap_push::MAX_SUBSCRIPTIONS_PER_APP {
        core.dispatch(req(
            "push.subscribe",
            &["demo", "kv.*", "Title", &format!("sub-{i}")],
        ))
        .unwrap();
    }
    let too_many = core
        .dispatch(req("push.subscribe", &["demo", "kv.set", "Title", "sub-extra"]))
        .unwrap_err();
    assert!(too_many.to_string().contains("at most 32 subscriptions"));
}

#[test]
fn push_runtime_resource_records_subscription_and_lists_it() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    grant_resource(&mut core, "demo", "push");

    let mut host = RuntimeResourceHost::new("demo", core.state().clone());
    let sub_id = host
        .call_resource(
            "push",
            "subscribe",
            &["kv.*".into(), "Changed {key}|{value}".into()],
        )
        .unwrap();
    assert!(matches!(sub_id, ReadValue::OptString(Some(_))));
    let records = host.take_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "push.subscribed");

    terrane_core::fold_records_in_memory(&mut core.state().clone(), &records).unwrap();
}

#[test]
fn app_removal_drops_push_state() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "push.subscribe",
        &["demo", "kv.*", "Changed", "sub-1"],
    ))
    .unwrap();
    core.dispatch(req(
        "push.record-delivery",
        &["demo", "sub-1", "2", "failed", "native unavailable"],
    ))
    .unwrap();

    core.dispatch(req("app.remove", &["demo"])).unwrap();
    assert!(!core.state().push.subscriptions.contains_key("demo"));
    assert!(!core.state().push.deliveries.contains_key("demo"));
    assert!(core.replay_matches().unwrap());
}
