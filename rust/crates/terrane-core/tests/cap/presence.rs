//! Engine tests for `presence`: durable channel definitions plus transient
//! publish calls that record nothing.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_core::{Core, Decision, Effect, EffectRunner, Error, EventRecord, State};

use crate::helpers::{grant_resource, req};

#[derive(Clone, Copy)]
struct PresenceRunner;

impl EffectRunner for PresenceRunner {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::PresencePublish { .. } => Ok(Vec::new()),
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
            r#"{{"id":"{app}","name":"{app}","runtime":"js","backend":"main.js","resources":["presence"]}}"#
        ),
    )
    .unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn define_drop_fold_and_replay() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, PresenceRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let records = core
        .dispatch(req(
            "presence.channel.define",
            &["demo", "cursor", "2048", "10"],
        ))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "presence.channel.defined");
    assert_eq!(core.state().presence.channels["demo"]["cursor"].max_payload, 2048);
    assert_eq!(core.state().presence.channels["demo"]["cursor"].max_rate, 10);

    let records = core
        .dispatch(req("presence.channel.drop", &["demo", "cursor"]))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "presence.channel.dropped");
    assert!(core.state().presence.channels.is_empty());
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().presence, core.state().presence);
}

#[test]
fn publish_decides_to_transient_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), PresenceRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let decision = core
        .decide(req(
            "presence.publish",
            &["demo", "cursor", r#"{"x":1}"#],
        ))
        .unwrap();
    assert!(matches!(
        decision,
        Decision::TransientEffect(Effect::PresencePublish { .. })
    ));
}

#[test]
fn resource_publish_records_nothing_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let backend = r#"
        function handle(input) {
            return ctx.resource.presence.publish("cursor", {x: 1, y: 2});
        }
    "#;
    let source = write_bundle(dir.path(), "presence-app", backend);
    let mut core = Core::open_with(&log, PresenceRunner).unwrap();
    core.dispatch(req(
        "app.add",
        &["presence-app", "Presence", "--source", &source],
    ))
    .unwrap();
    grant_resource(&mut core, "presence-app", "presence");

    let records = core
        .dispatch(req("js-runtime.run", &["presence-app", "go"]))
        .unwrap();
    assert!(records.is_empty(), "presence publish recorded: {records:?}");
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state(), core.state());
}

#[test]
fn validation_errors_are_typed() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), PresenceRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    assert_eq!(
        core.dispatch(req("presence.channel.define", &["ghost", "cursor"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    assert!(matches!(
        core.dispatch(req("presence.channel.define", &["demo", " bad "])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "presence.channel.define",
            &["demo", "cursor", "65537"],
        )),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.decide(req("presence.publish", &["demo", "cursor", "not-json"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn top_level_publish_is_refused_before_log_commit() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), PresenceRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let err = core
        .dispatch(req(
            "presence.publish",
            &["demo", "cursor", r#"{"x":1}"#],
        ))
        .unwrap_err();
    assert!(err.to_string().contains("transient effects are only valid"));
    assert!(core.replay_matches().unwrap());
}
