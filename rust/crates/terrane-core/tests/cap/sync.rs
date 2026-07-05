//! Engine tests for the `sync` capability: peer roster facts, cursor monotonicity,
//! replay identity, and the v2 event allowlist.

use tempfile::tempdir;
use terrane_cap_sync::{encode_batch_hex, SyncEnvelope};
use terrane_core::{Core, Error, QueryValue};

use crate::helpers::req;

#[test]
fn pair_unpair_and_cursor_queries_are_replayable() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("sync.pair", &["a1", "Laptop"])).unwrap();
    let peers = core.query("sync", "peers", &[]).unwrap();
    assert!(matches!(peers, QueryValue::Json(json) if json.contains("\"peer\":\"a1\"")));

    core.dispatch(req("sync.unpair", &["a1"])).unwrap();
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    let peers = reopened.query("sync", "peers", &[]).unwrap();
    assert!(matches!(peers, QueryValue::Json(json) if json.contains("\"paired\":false")));
}

#[test]
fn apply_records_foreign_kv_after_sync_applied_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("sync.pair", &["b0b", "Peer"])).unwrap();
    core.dispatch(req("kv.set", &["notes", "theme", "local"]))
        .unwrap();

    let batch = encode_batch_hex(&[
        kv_set("b0b", 4, "notes", "theme", "remote"),
        kv_set("b0b", 7, "notes", "lang", "en"),
    ])
    .unwrap();
    let records = core
        .dispatch(req("sync.apply", &["b0b", "notes", "4", "7", &batch]))
        .unwrap();

    assert_eq!(records[0].kind, "sync.applied");
    assert_eq!(records[1].kind, "kv.set");
    assert_eq!(core.state().kv.data["notes"]["theme"], "remote");
    assert_eq!(core.state().kv.data["notes"]["lang"], "en");
    assert_eq!(
        core.query("sync", "cursor", &["b0b".into(), "notes".into()])
            .unwrap(),
        QueryValue::U64(Some(7))
    );
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().kv.data["notes"]["theme"],
        "remote"
    );
}

#[test]
fn apply_validates_monotonic_cursor_allowlist_and_app_scope() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("app.add", &["other", "Other"])).unwrap();
    core.dispatch(req("sync.pair", &["cafe", "Peer"])).unwrap();

    let batch = encode_batch_hex(&[kv_set("cafe", 1, "notes", "theme", "dark")]).unwrap();
    core.dispatch(req("sync.apply", &["cafe", "notes", "1", "1", &batch]))
        .unwrap();
    let err = core
        .dispatch(req("sync.apply", &["cafe", "notes", "1", "1", &batch]))
        .unwrap_err();
    assert!(err.to_string().contains("cursor mismatch"), "{err}");

    let disallowed = encode_batch_hex(&[SyncEnvelope {
        origin_peer: "cafe".into(),
        origin_seq: 2,
        kind: "auth.granted".into(),
        payload: Vec::new(),
    }])
    .unwrap();
    let err = core
        .dispatch(req("sync.apply", &["cafe", "notes", "2", "2", &disallowed]))
        .unwrap_err();
    assert!(err.to_string().contains("not allowlisted"), "{err}");

    let wrong_app = encode_batch_hex(&[kv_set("cafe", 2, "other", "theme", "light")]).unwrap();
    let err = core
        .dispatch(req("sync.apply", &["cafe", "notes", "2", "2", &wrong_app]))
        .unwrap_err();
    assert!(err.to_string().contains("payload app"), "{err}");

    let unpaired = encode_batch_hex(&[kv_set("dead", 1, "notes", "theme", "x")]).unwrap();
    assert!(matches!(
        core.dispatch(req("sync.apply", &["dead", "notes", "1", "1", &unpaired])),
        Err(Error::InvalidInput(_))
    ));
}

fn kv_set(origin_peer: &str, origin_seq: u64, app: &str, key: &str, value: &str) -> SyncEnvelope {
    SyncEnvelope {
        origin_peer: origin_peer.to_string(),
        origin_seq,
        kind: "kv.set".to_string(),
        payload: terrane_cap_kv::set_event(app, key, value)
            .unwrap()
            .payload,
    }
}
