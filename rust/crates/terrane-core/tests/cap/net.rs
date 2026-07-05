//! Engine tests for the `net` capability — the recorded-effect mechanism, plus
//! the transient (unrecorded) `net.get` resource.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use std::collections::BTreeMap;

use terrane_cap_net::request::prepare_request;
use terrane_cap_net::{fetched_event, responded_event, RecordedBody};
use terrane_core::Error;
use terrane_core::{
    fold_records_in_memory, Core, Effect, EffectRunner, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

/// A runner that answers every HttpGet with one canned body — enough to exercise
/// the resource path without a network.
struct CannedGet(String);

impl EffectRunner for CannedGet {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                Ok(vec![fetched_event(app, url, 200, self.0.clone()).unwrap()])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

struct CannedRequest;

impl EffectRunner for CannedRequest {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpRequest { app, request } => {
                let prepared = prepare_request(request)?;
                Ok(vec![responded_event(
                    app,
                    prepared.request_key,
                    prepared.redacted_json,
                    201,
                    BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
                    RecordedBody {
                        kind: "inline".to_string(),
                        body: "{\"ok\":true}".to_string(),
                        is_base64: false,
                        hash: "a".repeat(64),
                        size: 11,
                        mime: "application/json".to_string(),
                    },
                )?])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

fn write_bundle(dir: &Path, name: &str, manifest: &str, backend: &str) -> String {
    let bundle = dir.join(name);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("manifest.json"), manifest).unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn net_get_resource_returns_body_but_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            if (input[0] === "get") return ctx.resource.net.get(input[1]);
            return "?";
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "fetcher",
        r#"{"id":"fetcher","name":"Fetcher","runtime":"js","backend":"main.js","resources":["net"]}"#,
        backend,
    );
    let mut core = Core::open_with(
        dir.path().join("log.bin"),
        CannedGet("035CD:12\r\n1E4C9:7\r\n".to_string()),
    )
    .unwrap();
    core.dispatch(req("app.add", &["fetcher", "Fetcher", "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "fetcher", "net"]))
        .unwrap();

    let records = core
        .dispatch(req(
            "js-runtime.run",
            &["fetcher", "get", "https://api.pwnedpasswords.com/range/ABCDE"],
        ))
        .unwrap();

    // The live body reaches the backend…
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("035CD:12\r\n1E4C9:7\r\n")
    );
    // …but the transient fetch records NOTHING: no event committed, and no
    // net.fetched folded into state — so the SHA-1 prefix that fetched it, and
    // the response, never enter the log.
    assert!(records.is_empty(), "net.get must record nothing, got: {records:?}");
    assert!(
        core.state().net.fetches.is_empty(),
        "net.get must not fold response into state"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn net_call_resource_returns_inline_body_but_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            if (input[0] === "call") return ctx.resource.net.call(input[1]);
            return "?";
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "caller",
        r#"{"id":"caller","name":"Caller","runtime":"js","backend":"main.js","resources":["net"]}"#,
        backend,
    );
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedRequest).unwrap();
    core.dispatch(req("app.add", &["caller", "Caller", "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "caller", "net"]))
        .unwrap();

    let records = core
        .dispatch(req(
            "js-runtime.run",
            &[
                "caller",
                "call",
                r#"{"method":"POST","url":"http://127.0.0.1/items","body":"{}"}"#,
            ],
        ))
        .unwrap();

    assert_eq!(core.take_last_output().as_deref(), Some("{\"ok\":true}"));
    assert!(
        records.is_empty(),
        "net.call must record nothing, got: {records:?}"
    );
    assert!(
        core.state().net.requests.is_empty(),
        "net.call must not fold response into state"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn fetched_event_folds_recorded_response_without_network() {
    let mut state = State::default();
    let records = vec![fetched_event(
        "notes",
        "http://127.0.0.1/data",
        200,
        "local response".to_string(),
    )
    .unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let resp = &state.net.fetches["notes"]["http://127.0.0.1/data"];
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "local response");
}

#[test]
fn responded_event_folds_redacted_request_and_replays_identically() {
    let mut state = State::default();
    let prepared = prepare_request(
        r#"{
            "method":"POST",
            "url":"http://127.0.0.1/items?token=query",
            "headers":{"Authorization":"Bearer secret","X-Trace":"ok"},
            "body":"{}",
            "responseBody":"inline"
        }"#,
    )
    .unwrap();
    let records = vec![responded_event(
        "notes",
        prepared.request_key.clone(),
        prepared.redacted_json,
        200,
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        RecordedBody {
            kind: "inline".to_string(),
            body: "{\"saved\":true}".to_string(),
            is_base64: false,
            hash: "b".repeat(64),
            size: 14,
            mime: "application/json".to_string(),
        },
    )
    .unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let response = &state.net.requests["notes"][&prepared.request_key];
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "{\"saved\":true}");
    assert!(!response.request_json_redacted.contains("Bearer secret"));
    let mut replayed = State::default();
    fold_records_in_memory(&mut replayed, &records).unwrap();
    assert_eq!(state, replayed);
}

#[test]
fn fetch_is_validated_purely_before_any_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    // A pure core (NoEffects): a valid Fetch reaches the runner and is refused…
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert!(matches!(
        core.dispatch(req("net.fetch", &["notes", "http://x/"])),
        Err(Error::InvalidInput(_))
    ));
    // …but a Fetch for a missing app is rejected in decide, before the runner.
    assert_eq!(
        core.dispatch(req("net.fetch", &["ghost", "http://x/"])),
        Err(Error::AppNotFound("ghost".into()))
    );
}
