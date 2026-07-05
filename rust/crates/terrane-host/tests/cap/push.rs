//! e2e smoke for local push delivery through the host edge.

use tempfile::tempdir;
use terrane_core::Request;
use terrane_host::{open_at_home, sync};

#[test]
fn push_delivery_queues_native_notification_and_records_outcome() {
    let dir = tempdir().unwrap();
    let mut core = open_at_home(dir.path()).unwrap();

    observe_notifications(&mut core);
    terrane_host::dispatch_on_core(
        &mut core,
        "app.add",
        &["notes".into(), "Notes".into()],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "push.subscribe",
        &[
            "notes".into(),
            "kv.*".into(),
            "Notes changed|{key}={value}".into(),
            "sub-1".into(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "kv.set",
        &["notes".into(), "theme".into(), "dark".into()],
    )
    .unwrap();

    let native = core.state().native.requests["notes"]
        .values()
        .find(|record| record.operation_id == "notification.show")
        .expect("notification request");
    assert_eq!(native.operation_id, "notification.show");
    assert!(native.input_json.contains("Notes changed"));
    assert!(native.input_json.contains("theme=dark"));
    assert_eq!(core.state().push.deliveries["notes"]["sub-1"].len(), 1);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn push_delivery_is_deduped_for_same_record() {
    let dir = tempdir().unwrap();
    let mut core = open_at_home(dir.path()).unwrap();

    observe_notifications(&mut core);
    terrane_host::dispatch_on_core(
        &mut core,
        "app.add",
        &["notes".into(), "Notes".into()],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "push.subscribe",
        &["notes".into(), "kv.*".into(), "Changed".into(), "sub-1".into()],
    )
    .unwrap();
    let records = terrane_host::dispatch_on_core(
        &mut core,
        "kv.set",
        &["notes".into(), "theme".into(), "dark".into()],
    )
    .unwrap()
    .records;
    terrane_host::push_watch::process_committed_records(&mut core, &records).unwrap();

    let notifications = core.state().native.requests["notes"]
        .values()
        .filter(|record| record.operation_id == "notification.show")
        .count();
    assert_eq!(notifications, 1);
}

#[test]
fn synced_push_subscription_delivers_on_target_home() {
    let a_dir = tempdir().unwrap();
    let b_dir = tempdir().unwrap();
    let mut a = open_at_home(a_dir.path()).unwrap();
    let mut b = open_at_home(b_dir.path()).unwrap();

    for core in [&mut a, &mut b] {
        observe_notifications(core);
        terrane_host::dispatch_on_core(
            core,
            "app.add",
            &["notes".into(), "Notes".into()],
        )
        .unwrap();
    }

    let a_peer = sync::local_peer_hex(&a).unwrap();
    let b_peer = sync::local_peer_hex(&b).unwrap();
    sync::pair_peer(&mut a, &b_peer, "B").unwrap();
    sync::pair_peer(&mut b, &a_peer, "A").unwrap();

    terrane_host::dispatch_on_core(
        &mut a,
        "push.subscribe",
        &["notes".into(), "kv.*".into(), "Changed {key}".into(), "sub-1".into()],
    )
    .unwrap();
    let batch = sync::event_batch_since(&a, "notes", 0).unwrap();
    sync::apply_event_batch(&mut b, "notes", &batch).unwrap();
    assert!(b.state().push.subscriptions["notes"].contains_key("sub-1"));

    terrane_host::dispatch_on_core(
        &mut a,
        "kv.set",
        &["notes".into(), "theme".into(), "dark".into()],
    )
    .unwrap();
    let cursor = match b.query("sync", "cursor", &[a_peer, "notes".into()]).unwrap() {
        terrane_core::QueryValue::U64(Some(value)) => value,
        _ => 0,
    };
    let batch = sync::event_batch_since(&a, "notes", cursor).unwrap();
    sync::apply_event_batch(&mut b, "notes", &batch).unwrap();

    let notifications = b.state().native.requests["notes"]
        .values()
        .filter(|record| record.operation_id == "notification.show")
        .count();
    assert_eq!(notifications, 1);
    assert_eq!(b.state().push.deliveries["notes"]["sub-1"].len(), 1);
}

#[test]
fn push_watcher_exposes_staleness_cutoff_constant() {
    assert_eq!(terrane_host::push_watch::stale_cutoff_ms(), 86_400_000);
}

fn observe_notifications(core: &mut terrane_host::HostCore) {
    core.dispatch(Request::trusted_host(
        "native.platform.observe",
        vec![
            "stub".into(),
            "test".into(),
            "push-test-1".into(),
            "notification.show".into(),
        ],
    ))
    .unwrap();
}
