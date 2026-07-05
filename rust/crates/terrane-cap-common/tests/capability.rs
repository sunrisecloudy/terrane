use std::any::Any;

use terrane_cap_common::{common_doc, prepare_send, CommonState};
use terrane_cap_interface::{StateStore, Error};

#[derive(Default)]
struct Store {
    common: CommonState,
    blob: terrane_cap_blob::BlobState,
    connection: terrane_cap_connection::ConnectionState,
    auth: terrane_cap_auth::AuthState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "common" => Some(&self.common),
            "blob" => Some(&self.blob),
            "connection" => Some(&self.connection),
            "auth" => Some(&self.auth),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "common" => Some(&mut self.common),
            "blob" => Some(&mut self.blob),
            "connection" => Some(&mut self.connection),
            "auth" => Some(&mut self.auth),
            _ => None,
        }
    }
}

#[test]
fn email_message_canonicalizes_defaults_and_hash_only_body() {
    let store = Store::default();
    let prepared = prepare_send(
        &store,
        "mailbot",
        r#"{"channel":"email","to":["a@example.com"],"subject":"Hi","text":"Hello"}"#,
    )
    .unwrap();

    assert_eq!(prepared.channel, "email");
    assert_eq!(prepared.connection, "smtp-default");
    assert_eq!(prepared.body_kind, "none");
    assert!(prepared.body.is_empty());
    assert_eq!(prepared.body_hash.len(), 64);
}

#[test]
fn record_body_true_inlines_small_body() {
    let store = Store::default();
    let prepared = prepare_send(
        &store,
        "mailbot",
        r#"{"channel":"email","to":["a@example.com"],"text":"Hello","recordBody":true}"#,
    )
    .unwrap();

    assert_eq!(prepared.body_kind, "inline");
    assert_eq!(prepared.body, "Hello");
}

#[test]
fn validation_rejects_unknown_channel_and_bad_recipient() {
    let store = Store::default();
    let err = prepare_send(
        &store,
        "mailbot",
        r#"{"channel":"sms","to":["a@example.com"],"text":"Hello"}"#,
    )
    .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(message) if message.contains("unknown")));

    let err = prepare_send(
        &store,
        "mailbot",
        r#"{"channel":"email","to":["not-an-email"],"text":"Hello"}"#,
    )
    .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(message) if message.contains("recipient")));
}

#[test]
fn doc_lists_channel_limits() {
    let doc = common_doc(true);
    assert_eq!(doc.namespace, "common");
    assert!(doc.manifest.commands.contains(&"common.send".to_string()));
    assert!(doc.manifest.events.contains(&"common.sent".to_string()));
    assert!(doc
        .limits
        .iter()
        .any(|limit| limit.name == "emailSendsPerHour"));
    assert!(!doc.internal.is_empty());
}
