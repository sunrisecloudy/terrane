use tempfile::tempdir;

use terrane_cap_connection::{defined_event, ConnectionStatus};
use terrane_cap_net::request::prepare_request;
use terrane_cap_net::{responded_event, RecordedBody};
use terrane_core::{
    fold_records_in_memory, Core, Effect, EffectRunner, Error, EventRecord, State,
    LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

struct MarkerRunner;

impl EffectRunner for MarkerRunner {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpRequest { app, request } => {
                let prepared = prepare_request(request)?;
                assert!(prepared.has_unresolved_secret);
                Ok(vec![responded_event(
                    app,
                    prepared.request_key,
                    prepared.redacted_json,
                    200,
                    Default::default(),
                    RecordedBody {
                        kind: "inline".to_string(),
                        body: "ok".to_string(),
                        is_base64: false,
                        hash: "c".repeat(64),
                        size: 2,
                        mime: "text/plain".to_string(),
                    },
                )?])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn connection_metadata_folds_and_replays_without_secret_material() {
    let records = vec![
        defined_event("github", "apiKey", "{}").unwrap(),
        terrane_cap_connection::authorized_event(
            "github",
            vec!["repo".to_string()],
            "2030-01-01T00:00:00Z",
        )
        .unwrap(),
    ];
    let mut state = State::default();
    fold_records_in_memory(&mut state, &records).unwrap();
    let mut replayed = State::default();
    fold_records_in_memory(&mut replayed, &records).unwrap();
    assert_eq!(state, replayed);
    assert_eq!(
        terrane_cap_connection::all_statuses(&state).unwrap(),
        vec![ConnectionStatus {
            name: "github".to_string(),
            kind: "apiKey".to_string(),
            authorized: true,
            scopes: vec!["repo".to_string()],
            expires_at: Some("2030-01-01T00:00:00Z".to_string()),
        }]
    );
    assert!(!format!("{records:?}").contains("raw-secret"));
}

#[test]
fn net_request_records_secret_marker_verbatim_for_stable_request_identity() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), MarkerRunner).unwrap();
    core.dispatch(req("app.add", &["web", "Web"])).unwrap();
    core.dispatch(req("connection.define", &["github", "apiKey", "{}"]))
        .unwrap();
    core.dispatch(req(
        "auth.grant",
        &[LOCAL_OWNER_SUBJECT, "web", "connection:github"],
    ))
    .unwrap();

    let request = r#"{"url":"http://127.0.0.1/","headers":{"authorization":{"$secret":"github"}}}"#;
    let records = core
        .dispatch(req("net.request", &["web", request]))
        .unwrap();

    let log_text = format!("{records:?}");
    assert!(log_text.contains("net.responded"));
    assert!(!log_text.contains("raw-secret"));
    let folded = core
        .state()
        .net
        .requests
        .get("web")
        .and_then(|requests| requests.values().next())
        .unwrap();
    assert!(folded.request_json_redacted.contains("$secret"));
    assert!(core.replay_matches().unwrap());
}
