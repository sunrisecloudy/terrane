//! e2e smoke for sync v2 host helpers over two real temp homes.

use tempfile::tempdir;
use terrane_cap_blob::live_hashes_for_app;
use terrane_host::{blob_store, open_at_home, sync};
use terrane_core::Request;

#[test]
fn sync_v2_two_homes_converge_kv_crdt_and_blob_refs() {
    let a_dir = tempdir().unwrap();
    let b_dir = tempdir().unwrap();
    let mut a = open_at_home(a_dir.path()).unwrap();
    let mut b = open_at_home(b_dir.path()).unwrap();

    for core in [&mut a, &mut b] {
        core.dispatch(Request::trusted_host(
            "app.add",
            vec!["notes".into(), "Notes".into()],
        ))
        .unwrap();
    }

    let a_peer = sync::local_peer_hex(&a).unwrap();
    let b_peer = sync::local_peer_hex(&b).unwrap();
    sync::pair_peer(&mut a, &b_peer, "B").unwrap();
    sync::pair_peer(&mut b, &a_peer, "A").unwrap();

    a.dispatch(Request::trusted_host(
        "kv.set",
        vec!["notes".into(), "theme".into(), "dark".into()],
    ))
    .unwrap();
    b.dispatch(Request::trusted_host(
        "kv.set",
        vec!["notes".into(), "lang".into(), "en".into()],
    ))
    .unwrap();
    a.dispatch(Request::trusted_host(
        "crdt.listPush",
        vec!["notes".into(), "todos".into(), "from-a".into()],
    ))
    .unwrap();
    b.dispatch(Request::trusted_host(
        "crdt.listPush",
        vec!["notes".into(), "todos".into(), "from-b".into()],
    ))
    .unwrap();
    a.dispatch(Request::trusted_host(
        "blob.put",
        vec![
            "notes".into(),
            "hello.txt".into(),
            "text/plain".into(),
            "aGVsbG8=".into(),
        ],
    ))
    .unwrap();

    exchange_once(&mut a, &mut b);
    exchange_once(&mut b, &mut a);
    sync::apply_blob_refs(&mut b, "notes", &sync::blob_refs(&a, "notes")).unwrap();
    let hashes = live_hashes_for_app(&a.state().blob, "notes");
    let copied = blob_store::copy_hashes_from_home(b_dir.path(), a_dir.path(), &hashes).unwrap();

    assert_eq!(a.state().kv.data["notes"]["lang"], "en");
    assert_eq!(b.state().kv.data["notes"]["theme"], "dark");
    assert_eq!(a.state().crdt.docs["notes"].get_deep_value(), b.state().crdt.docs["notes"].get_deep_value());
    assert_eq!(b.state().blob.blobs["notes"]["hello.txt"].hash, hashes[0]);
    assert_eq!(copied, 1);
    assert!(a.replay_matches().unwrap());
    assert!(b.replay_matches().unwrap());
}

fn exchange_once(source: &mut terrane_host::HostCore, target: &mut terrane_host::HostCore) {
    let vv = sync::vv_response(source, "notes", &terrane_cap_crdt::crdt_vv(target.state(), "notes"))
        .unwrap();
    sync::ingest_crdt_delta(target, "notes", &vv.delta).unwrap();
    let back = terrane_cap_crdt::crdt_export_from_vv(target.state(), "notes", &vv.vv).unwrap();
    sync::ingest_crdt_delta(source, "notes", &back).unwrap();

    let source_peer = sync::local_peer_hex(source).unwrap();
    let cursor = match target
        .query("sync", "cursor", &[source_peer, "notes".into()])
        .unwrap()
    {
        terrane_core::QueryValue::U64(Some(value)) => value,
        _ => 0,
    };
    let batch = sync::event_batch_since(source, "notes", cursor).unwrap();
    sync::apply_event_batch(target, "notes", &batch).unwrap();
}
