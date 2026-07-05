//! Engine tests for the `telemetry` capability: recorded error facts, transient
//! log decisions, replay identity, truncation, and app-removal cleanup.

use std::cell::RefCell;

use tempfile::tempdir;
use terrane_cap_telemetry::{
    decode_error_event, error_event, sha256_hex, ErrorFact, MAX_DATA_BYTES, MAX_MSG_BYTES,
};
use terrane_core::{Core, Decision, Effect, EffectRunner, EventRecord, State};

use crate::helpers::req;

#[derive(Default)]
struct LogRunner {
    lines: RefCell<Vec<(String, String, String, String)>>,
}

impl EffectRunner for LogRunner {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::AppLog {
                app,
                level,
                msg,
                data,
            } => {
                self.lines.borrow_mut().push((
                    app.clone(),
                    level.clone(),
                    msg.clone(),
                    data.clone(),
                ));
                if level == "error" {
                    Ok(vec![error_event(
                        app,
                        terrane_cap_telemetry::SOURCE_EXPLICIT,
                        msg,
                        "",
                        data,
                    )?])
                } else {
                    Ok(Vec::new())
                }
            }
            other => Err(terrane_core::Error::Runtime(format!(
                "unexpected effect: {other:?}"
            ))),
        }
    }
}

#[test]
fn telemetry_error_records_state_and_replays() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), LogRunner::default()).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let records = core
        .dispatch(req(
            "telemetry.error",
            &["notes", "render failed", r#"{"screen":"home"}"#],
        ))
        .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "telemetry.error");
    assert_eq!(core.state().telemetry.error_count["notes"], 1);
    assert_eq!(
        core.state().telemetry.last_errors["notes"][0],
        ErrorFact {
            app: "notes".into(),
            source: "explicit".into(),
            message: "render failed".into(),
            stack: String::new(),
            data_digest: sha256_hex(br#"{"screen":"home"}"#),
        }
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn telemetry_debug_is_transient_and_top_level_rejected() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), LogRunner::default()).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let decision = core
        .decide(req("telemetry.debug", &["notes", "only buffer"]))
        .unwrap();
    assert!(matches!(decision, Decision::TransientEffect(Effect::AppLog { .. })));

    let err = core
        .dispatch(req("telemetry.debug", &["notes", "only buffer"]))
        .unwrap_err();
    assert!(matches!(err, terrane_core::Error::InvalidInput(_)));
    assert!(core.state().telemetry.error_count.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn telemetry_truncates_message_and_data_digest_uses_truncated_data() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), LogRunner::default()).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let msg = "m".repeat(MAX_MSG_BYTES + 10);
    let data = "d".repeat(MAX_DATA_BYTES + 10);
    let records = core
        .dispatch(req("telemetry.error", &["notes", &msg, &data]))
        .unwrap();
    let fact = decode_error_event(&records[0]).unwrap();

    assert_eq!(fact.message.chars().count(), MAX_MSG_BYTES + 3);
    assert!(fact.message.ends_with("..."));
    let truncated_data = format!("{}...", "d".repeat(MAX_DATA_BYTES));
    assert_eq!(fact.data_digest, sha256_hex(truncated_data.as_bytes()));
}

#[test]
fn telemetry_rejects_invalid_error_source() {
    let err = error_event("notes", "remote-export", "nope", "", "{}").unwrap_err();
    assert!(matches!(err, terrane_core::Error::InvalidInput(_)));
}

#[test]
fn telemetry_app_removed_drops_app_slice() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), LogRunner::default()).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("telemetry.error", &["notes", "boom"]))
        .unwrap();
    assert!(core.state().telemetry.error_count.contains_key("notes"));

    core.dispatch(req("app.remove", &["notes"])).unwrap();

    assert!(!core.state().telemetry.error_count.contains_key("notes"));
    assert!(!core.state().telemetry.last_errors.contains_key("notes"));
    assert!(core.replay_matches().unwrap());
}
