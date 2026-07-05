use std::collections::BTreeMap;

use tempfile::tempdir;
use terrane_core::{Core, Effect, EffectRunner, Error, EventRecord, Result, State};

use crate::helpers::req;

struct WebhookRunner;

impl EffectRunner for WebhookRunner {
    fn run(&self, effect: &Effect, state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::WebhookRegister { app, name, verb } => {
                let existing = state
                    .webhook
                    .routes
                    .get(app)
                    .and_then(|routes| routes.get(name))
                    .is_some();
                let token = if existing {
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                } else {
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                };
                let event = if existing {
                    terrane_cap_webhook::rotated_event(app, name, verb, token)?
                } else {
                    terrane_cap_webhook::registered_event(app, name, verb, token)?
                };
                Ok(vec![event])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn webhook_register_rotate_ingest_redacts_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, WebhookRunner).unwrap();
    core.dispatch(req("app.add", &["receiver", "Receiver"]))
        .unwrap();

    core.dispatch(req(
        "webhook.register",
        &["receiver", "github", "receive"],
    ))
    .unwrap();
    assert_eq!(
        core.state().webhook.routes["receiver"]["github"].token,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(core.state().webhook.routes["receiver"]["github"].verb, "receive");

    let delivery = r#"{"app":"receiver","name":"github","token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","method":"POST","headers":{"Authorization":"Bearer secret","X-Hub-Signature-256":"sha256=abc","X-Api-Key":"plain-secret"},"body":"{\"ok\":true}","body_mime":"application/json","received_at":123}"#;
    let records = core
        .dispatch(req("webhook.ingest", &[delivery]))
        .unwrap();
    assert_eq!(records[0].kind, "webhook.received");
    let received = terrane_cap_webhook::decode_delivery(&records[0]).unwrap();
    assert_eq!(
        received.headers,
        BTreeMap::from([
            ("authorization".to_string(), "«redacted»".to_string()),
            ("x-api-key".to_string(), "«redacted»".to_string()),
            ("x-hub-signature-256".to_string(), "sha256=abc".to_string()),
        ])
    );
    assert_eq!(received.body_kind, "inline");
    assert_eq!(received.body_size, 11);
    assert_eq!(core.state().webhook.deliveries["receiver"]["github"], 1);

    core.dispatch(req("webhook.rotate", &["receiver", "github"]))
        .unwrap();
    assert_eq!(
        core.state().webhook.routes["receiver"]["github"].token,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    let bad = delivery.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab",
    );
    assert!(matches!(
        core.dispatch(req("webhook.ingest", &[&bad])),
        Err(Error::InvalidInput(_))
    ));

    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().webhook, core.state().webhook);
}

#[test]
fn webhook_validation_and_app_removed_are_replay_safe() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, WebhookRunner).unwrap();
    core.dispatch(req("app.add", &["receiver", "Receiver"]))
        .unwrap();

    assert!(matches!(
        core.dispatch(req("webhook.register", &["receiver", "BadName", "receive"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("webhook.register", &["ghost", "github", "receive"])),
        Err(Error::AppNotFound(_))
    ));

    core.dispatch(req(
        "webhook.register",
        &["receiver", "github", "receive"],
    ))
    .unwrap();
    assert!(core.state().webhook.routes.contains_key("receiver"));
    core.dispatch(req("app.remove", &["receiver"])).unwrap();
    assert!(!core.state().webhook.routes.contains_key("receiver"));
    assert!(core.replay_matches().unwrap());
}
