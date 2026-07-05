//! Engine tests for the `stream` capability.

use tempfile::tempdir;
use terrane_core::Core;

use crate::helpers::{public_req, req};

fn open_stream(core: &mut Core, app: &str, name: &str) {
    let request = r#"{"kind":"sse","url":"https://example.test/feed?token=secret","headers":{"Authorization":"Bearer raw","X-Trace":"ok"},"sensitiveHeaders":["x-trace"]}"#;
    core.dispatch(req("stream.open", &[app, name, "onMessage", request]))
        .unwrap();
}

#[test]
fn stream_open_redacts_request_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();

    let records = core
        .dispatch(req(
            "stream.open",
            &[
                "prices",
                "btc",
                "onMessage",
                r#"{"kind":"sse","url":"https://example.test/feed?token=secret","headers":{"Authorization":"Bearer raw","X-Trace":"ok"},"sensitiveHeaders":["x-trace"]}"#,
            ],
        ))
        .unwrap();

    assert_eq!(records[0].kind, "stream.opened");
    let meta = &core.state().stream.streams["prices"]["btc"];
    assert_eq!(meta.verb, "onMessage");
    assert_eq!(meta.kind.as_str(), "sse");
    assert!(meta.request_json_redacted.contains("«redacted»"));
    assert!(!meta.request_json_redacted.contains("Bearer raw"));
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().stream, core.state().stream);
}

#[test]
fn stream_messages_are_monotonic_and_replay_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();
    open_stream(&mut core, "prices", "btc");

    let hash = terrane_cap_stream::sha256_hex(b"tick");
    core.dispatch(req(
        "stream.message",
        &["prices", "btc", "1", "inline", "tick", "false", &hash, "4", "1000"],
    ))
    .unwrap();
    assert_eq!(core.state().stream.streams["prices"]["btc"].last_seq, 1);
    assert_eq!(core.state().stream.messages["prices"]["btc"].data, "tick");

    let err = core
        .dispatch(req(
            "stream.message",
            &["prices", "btc", "1", "inline", "again", "false", &hash, "5", "1001"],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("seq regression"), "{err}");
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().stream, core.state().stream);
}

#[test]
fn stream_reopened_and_closed_fold_without_restreaming() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();
    open_stream(&mut core, "prices", "btc");
    core.dispatch(req("stream.reopened", &["prices", "btc", "0", "1"]))
        .unwrap();
    core.dispatch(req("stream.close-host", &["prices", "btc", "remote"]))
        .unwrap();

    let meta = &core.state().stream.streams["prices"]["btc"];
    assert_eq!(meta.status, terrane_cap_stream::StreamStatus::Closed);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn stream_validation_and_trusted_ingest_errors_are_typed() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();

    let err = core
        .dispatch(req(
            "stream.open",
            &["prices", "BadName", "onMessage", r#"{"kind":"sse","url":"https://example.test"}"#],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("[a-z0-9-_]"), "{err}");

    let err = core
        .dispatch(req(
            "stream.open",
            &["prices", "bad", "onMessage", r#"{"kind":"ws","url":"https://example.test"}"#],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not support URL scheme"), "{err}");

    open_stream(&mut core, "prices", "btc");
    let hash = terrane_cap_stream::sha256_hex(b"tick");
    let err = core
        .dispatch(public_req(
            "stream.message",
            &["prices", "btc", "1", "inline", "tick", "false", &hash, "4", "1000"],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("requires trusted host authority"), "{err}");
}

#[test]
fn stream_app_removed_drops_state() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();
    open_stream(&mut core, "prices", "btc");
    let hash = terrane_cap_stream::sha256_hex(b"tick");
    core.dispatch(req(
        "stream.message",
        &["prices", "btc", "1", "inline", "tick", "false", &hash, "4", "1000"],
    ))
    .unwrap();

    core.dispatch(req("app.remove", &["prices"])).unwrap();
    assert!(!core.state().stream.streams.contains_key("prices"));
    assert!(!core.state().stream.messages.contains_key("prices"));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn stream_open_limit_is_enforced() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["prices", "Prices"])).unwrap();
    for i in 0..terrane_cap_stream::MAX_OPEN_STREAMS_PER_APP {
        open_stream(&mut core, "prices", &format!("s{i}"));
    }
    let err = core
        .dispatch(req(
            "stream.open",
            &[
                "prices",
                "overflow",
                "onMessage",
                r#"{"kind":"sse","url":"https://example.test/feed"}"#,
            ],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("open streams"), "{err}");
}
