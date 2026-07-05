//! Engine tests for `tts`: transient playback, recorded render artifacts, folded
//! render metadata, validation, truncation, and replay identity.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_tts::{rendered_event, sha256_hex, RenderRecord, TTS_MIME_WAV};
use terrane_core::{
    fold_records_in_memory, Capability, Core, Effect, EffectRunner, Error, EventRecord, State,
};

use crate::helpers::{grant_resource, req};

#[derive(Clone, Copy)]
struct TtsRunner;

impl EffectRunner for TtsRunner {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::TtsSpeak { .. } => Ok(Vec::new()),
            Effect::TtsRender {
                app,
                text,
                text_hash,
                voice,
                rate_milli,
            } => {
                let bytes = format!("wav:{text}:{voice:?}:{rate_milli}").into_bytes();
                let blob_hash = sha256_hex(&bytes);
                Ok(vec![
                    terrane_cap_blob::stored_event(
                        app,
                        format!("__tts__/{text_hash}"),
                        &blob_hash,
                        u64::try_from(bytes.len())
                            .map_err(|_| Error::Storage("tts test bytes overflow".into()))?,
                        TTS_MIME_WAV,
                    )?,
                    rendered_event(&RenderRecord {
                        app: app.clone(),
                        text_hash: text_hash.clone(),
                        voice: voice.clone(),
                        rate_milli: *rate_milli,
                        blob_hash,
                        size: u64::try_from(bytes.len())
                            .map_err(|_| Error::Storage("tts test bytes overflow".into()))?,
                        mime: TTS_MIME_WAV.to_string(),
                        duration_ms: 123,
                    })?,
                ])
            }
            other => Err(Error::InvalidInput(format!("unexpected effect: {other:?}"))),
        }
    }
}

fn write_bundle(dir: &Path, app: &str, backend: &str) -> String {
    let bundle = dir.join(app);
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{"id":"{app}","name":"{app}","runtime":"js","backend":"main.js","resources":["tts"]}}"#
        ),
    )
    .unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn decide_shapes_are_transient_for_speak_and_recorded_for_render() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), TtsRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let speak = core
        .decide(req("tts.speak", &["demo", "hello"]))
        .unwrap();
    assert!(matches!(
        speak,
        terrane_core::Decision::TransientEffect(Effect::TtsSpeak { .. })
    ));

    let render = core
        .decide(req("tts.render", &["demo", "--voice", "Alex", "--rate", "1250", "hello"]))
        .unwrap();
    assert!(matches!(
        render,
        terrane_core::Decision::Effect(Effect::TtsRender { .. })
    ));
}

#[test]
fn render_records_blob_and_tts_metadata_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, TtsRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let records = core
        .dispatch(req("tts.render", &["demo", "--voice", "Alex", "hello world"]))
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, "blob.stored");
    assert_eq!(records[1].kind, "tts.rendered");

    let text_hash = sha256_hex("hello world".as_bytes());
    let render = &core.state().tts.renders["demo"][&text_hash];
    assert_eq!(render.voice.as_deref(), Some("Alex"));
    assert_eq!(render.rate_milli, 1000);
    assert_eq!(render.mime, TTS_MIME_WAV);
    assert_eq!(render.duration_ms, 123);
    assert!(core.state().blob.blobs["demo"].contains_key(&format!("__tts__/{text_hash}")));
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().tts, core.state().tts);
}

#[test]
fn speak_resource_returns_ok_and_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.tts.speak("hello");
        }
    "#;
    let source = write_bundle(dir.path(), "speaker", backend);
    let mut core = Core::open_with(dir.path().join("log.bin"), TtsRunner).unwrap();
    core.dispatch(req("app.add", &["speaker", "Speaker", "--source", &source]))
        .unwrap();
    grant_resource(&mut core, "speaker", "tts");

    let records = core.dispatch(req("js-runtime.run", &["speaker", "go"])).unwrap();
    assert!(records.is_empty(), "tts.speak must record nothing: {records:?}");
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    assert!(core.state().tts.renders.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn render_resource_returns_json_and_records_artifact() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.tts.render("hello from app", "--rate", "1500");
        }
    "#;
    let source = write_bundle(dir.path(), "reader", backend);
    let mut core = Core::open_with(dir.path().join("log.bin"), TtsRunner).unwrap();
    core.dispatch(req("app.add", &["reader", "Reader", "--source", &source]))
        .unwrap();
    grant_resource(&mut core, "reader", "tts");

    let records = core.dispatch(req("js-runtime.run", &["reader", "go"])).unwrap();
    assert_eq!(records.iter().filter(|r| r.kind == "tts.rendered").count(), 1);
    let output = core.take_last_output().unwrap();
    assert!(output.contains("\"blobHash\""), "output: {output}");
    assert!(output.contains("\"rateMilli\":1500"), "output: {output}");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn validation_errors_are_typed() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), TtsRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    assert_eq!(
        core.dispatch(req("tts.render", &["ghost", "hello"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    assert!(matches!(
        core.dispatch(req("tts.render", &["demo", "--rate", "499", "hello"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("tts.render", &["demo", "--voice", "bad voice", "hello"])),
        Err(Error::InvalidInput(_))
    ));
    let too_long = "x".repeat(terrane_cap_tts::MAX_TEXT_BYTES + 1);
    assert!(matches!(
        core.dispatch(req("tts.render", &["demo", &too_long])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn fold_keeps_last_100_renders_per_app_and_app_removed_clears() {
    let mut state = State::default();
    let mut records = Vec::new();
    for i in 0..105 {
        records.push(
            rendered_event(&RenderRecord {
                app: "demo".to_string(),
                text_hash: format!("{i:064x}"),
                voice: None,
                rate_milli: 1000,
                blob_hash: format!("{:064x}", i + 1000),
                size: 10,
                mime: TTS_MIME_WAV.to_string(),
                duration_ms: 1,
            })
            .unwrap(),
        );
    }
    fold_records_in_memory(&mut state, &records).unwrap();
    assert_eq!(state.tts.renders["demo"].len(), 100);
    assert!(!state.tts.renders["demo"].contains_key(&format!("{:064x}", 0)));
    assert!(state.tts.renders["demo"].contains_key(&format!("{:064x}", 104)));

    let removed = make_app_removed("demo");
    fold_records_in_memory(&mut state, std::slice::from_ref(&removed)).unwrap();
    assert!(state.tts.renders.is_empty());
    assert!(state.tts.order.is_empty());
}

#[test]
fn describe_never_prints_source_text() {
    let record = rendered_event(&RenderRecord {
        app: "demo".to_string(),
        text_hash: sha256_hex("secret words".as_bytes()),
        voice: Some("Alex".to_string()),
        rate_milli: 1000,
        blob_hash: "a".repeat(64),
        size: 1,
        mime: TTS_MIME_WAV.to_string(),
        duration_ms: 9,
    })
    .unwrap();
    let description = terrane_cap_tts::TtsCapability.describe(&record).unwrap();
    assert!(description.contains("voice=Alex"));
    assert!(description.contains("duration_ms=9"));
    assert!(!description.contains("secret"));
}

fn make_app_removed(id: &str) -> EventRecord {
    #[derive(borsh::BorshSerialize)]
    struct AppRemoved {
        id: String,
    }
    terrane_cap_interface::encode_event("app.removed", &AppRemoved { id: id.to_string() }).unwrap()
}
