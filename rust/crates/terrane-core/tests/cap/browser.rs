//! Engine tests for the `browser` capability — recorded render effects plus
//! the transient unrecorded `browser.peek` resource.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_browser::request::prepare_render;
use terrane_cap_browser::{rendered_event, RecordedBody, RenderedEvent};
use terrane_core::Error;
use terrane_core::{
    fold_records_in_memory, Core, Effect, EffectRunner, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

struct CannedBrowser;

impl EffectRunner for CannedBrowser {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::BrowserRender { app, request } => {
                let prepared = prepare_render(request)?;
                Ok(vec![rendered_event(RenderedEvent {
                    app: app.to_string(),
                    request_key: prepared.request_key,
                    request_json_redacted: prepared.redacted_json,
                    url: prepared.url,
                    output: prepared.output.as_str().to_string(),
                    status: 200,
                    body: RecordedBody {
                        kind: "inline".to_string(),
                        body: "Rendered by JS".to_string(),
                        hash: "b".repeat(64),
                        size: 14,
                        mime: prepared.output.mime().to_string(),
                    },
                    title: "Rendered".to_string(),
                })?])
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
fn browser_render_resource_records_and_replays_identically() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.browser.render(JSON.stringify({url: input[1], output: "text"}));
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "renderer",
        r#"{"id":"renderer","name":"Renderer","runtime":"js","backend":"main.js","resources":["browser"]}"#,
        backend,
    );
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedBrowser).unwrap();
    core.dispatch(req("app.add", &["renderer", "Renderer", "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "renderer", "browser"]))
        .unwrap();

    let records = core
        .dispatch(req("js-runtime.run", &["renderer", "render", "https://example.test/app"]))
        .unwrap();

    assert_eq!(core.take_last_output().as_deref(), Some("Rendered by JS"));
    assert!(records.iter().any(|record| record.kind == "browser.rendered"));
    assert_eq!(core.state().browser.renders["renderer"].len(), 1);
    assert!(core.replay_matches().unwrap());

    let mut replay = State::default();
    fold_records_in_memory(&mut replay, &records).unwrap();
    assert_eq!(replay.browser.renders["renderer"].len(), 1);
}

#[test]
fn browser_peek_resource_returns_body_but_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.browser.peek(JSON.stringify({url: input[1], output: "text"}));
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "peeker",
        r#"{"id":"peeker","name":"Peeker","runtime":"js","backend":"main.js","resources":["browser"]}"#,
        backend,
    );
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedBrowser).unwrap();
    core.dispatch(req("app.add", &["peeker", "Peeker", "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "peeker", "browser"]))
        .unwrap();

    let records = core
        .dispatch(req("js-runtime.run", &["peeker", "peek", "https://example.test/app"]))
        .unwrap();

    assert_eq!(core.take_last_output().as_deref(), Some("Rendered by JS"));
    assert!(records.is_empty(), "browser.peek must record nothing: {records:?}");
    assert!(core.state().browser.renders.is_empty());
    assert!(core.replay_matches().unwrap());
}
