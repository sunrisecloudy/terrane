use tempfile::{tempdir, TempDir};

use terrane_cap_common::{decode_prepared_send, sent_event};
use terrane_core::{
    Core, Effect, EffectRunner, Error, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

struct CannedSend;

impl EffectRunner for CannedSend {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::ChannelSend {
                app,
                channel: _,
                message,
            } => {
                let prepared = decode_prepared_send(message)?;
                Ok(vec![sent_event(
                    app,
                    &prepared,
                    "msg-1@example.test",
                    "sent",
                    "",
                    prepared.sent_at.unwrap_or(1),
                )?])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

fn granted_core() -> (TempDir, Core<CannedSend>) {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedSend).unwrap();
    core.dispatch(req("app.add", &["mailbot", "Mail Bot"]))
        .unwrap();
    core.dispatch(req(
        "auth.grant",
        &[LOCAL_OWNER_SUBJECT, "mailbot", "common:send:email"],
    ))
    .unwrap();
    (dir, core)
}

#[test]
fn common_send_records_redacted_outcome_and_replays() {
    let (_dir, mut core) = granted_core();
    let records = core
        .dispatch(req(
            "common.send",
            &[
                "mailbot",
                r#"{"channel":"email","to":["a@example.com"],"bcc":["b@example.com"],"subject":"Hi","text":"secret body","sentAt":10}"#,
            ],
        ))
        .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "common.sent");
    let meta = core.state().common.sent["mailbot"]["msg-1@example.test"].clone();
    assert_eq!(meta.channel, "email");
    assert_eq!(meta.to_count, 2);
    assert_eq!(meta.subject.as_deref(), Some("Hi"));
    assert_eq!(meta.status, "sent");
    assert!(!format!("{records:?}").contains("secret body"));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn missing_channel_grant_blocks_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedSend).unwrap();
    core.dispatch(req("app.add", &["mailbot", "Mail Bot"]))
        .unwrap();
    let err = core
        .dispatch(req(
            "common.send",
            &[
                "mailbot",
                r#"{"channel":"email","to":["a@example.com"],"text":"Hello"}"#,
            ],
        ))
        .unwrap_err();
    assert!(err.to_string().contains("common:send:email"));
}

#[test]
fn email_rate_limit_counts_recorded_attempts() {
    let (_dir, mut core) = granted_core();
    for i in 0..terrane_cap_common::MAX_EMAIL_SENDS_PER_HOUR {
        let msg = format!(
            r#"{{"channel":"email","to":["a@example.com"],"text":"Hello","sentAt":{}}}"#,
            1000 + i
        );
        core.dispatch(req("common.send", &["mailbot", &msg])).unwrap();
    }

    let err = core
        .dispatch(req(
            "common.send",
            &[
                "mailbot",
                r#"{"channel":"email","to":["a@example.com"],"text":"Hello","sentAt":1200}"#,
            ],
        ))
        .unwrap_err();
    assert!(err.to_string().contains("20/hour"));
}

#[test]
fn app_removed_clears_common_state() {
    let (_dir, mut core) = granted_core();
    core.dispatch(req(
        "common.send",
        &[
            "mailbot",
            r#"{"channel":"email","to":["a@example.com"],"text":"Hello","sentAt":10}"#,
        ],
    ))
    .unwrap();
    assert!(!core.state().common.sent["mailbot"].is_empty());
    core.dispatch(req("app.remove", &["mailbot"])).unwrap();
    assert!(!core.state().common.sent.contains_key("mailbot"));
    assert!(!core.state().common.attempts.contains_key("mailbot"));
    assert!(core.replay_matches().unwrap());
}
