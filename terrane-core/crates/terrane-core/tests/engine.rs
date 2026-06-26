//! Integration tests for the terrane-core engine, driven entirely through its
//! public surface (`Core`, `replay`) — kept out of the implementation file so
//! the engine reads as one thing and its proofs as another.

use tempfile::tempdir;
use terrane_core::{replay, Core, Effect, EffectRunner};
use terrane_domain::{Command, Error, Event, Result};

fn add(id: &str, name: &str) -> Command {
    Command::AddApp {
        id: id.into(),
        name: name.into(),
        source: None,
    }
}

#[test]
fn executes_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open(&log).unwrap();
    core.execute(add("notes", "Notes")).unwrap();
    core.execute(add("tasks", "Task Workbench")).unwrap();
    core.execute(Command::RemoveApp { id: "notes".into() })
        .unwrap();

    // The in-memory State must equal a fresh replay of the log.
    assert!(core.replay_matches().unwrap());
    let replayed = replay(&log).unwrap();
    assert_eq!(replayed.apps.len(), 1);
    assert!(replayed.apps.contains_key("tasks"));

    // A brand-new Core opened on the same log rebuilds the same world.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), &replayed);
}

#[test]
fn source_round_trips_through_the_log() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.execute(Command::AddApp {
        id: "notes".into(),
        name: "Notes".into(),
        source: Some("apps/notes".into()),
    })
    .unwrap();
    let reopened = Core::open(&log).unwrap();
    assert_eq!(
        reopened.state().apps["notes"].source.as_deref(),
        Some("apps/notes")
    );
}

#[test]
fn rejects_duplicate_and_missing() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.execute(add("notes", "Notes")).unwrap();
    assert_eq!(
        core.execute(add("notes", "Notes Again")),
        Err(Error::AppExists("notes".into()))
    );
    assert_eq!(
        core.execute(Command::RemoveApp { id: "ghost".into() }),
        Err(Error::AppNotFound("ghost".into()))
    );

    // Rejected commands wrote nothing: still exactly one app.
    assert_eq!(core.state().apps.len(), 1);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn kv_resource_records_and_cascades() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.execute(add("notes", "Notes")).unwrap();

    // Writing to an app that doesn't exist is rejected.
    assert_eq!(
        core.execute(Command::KvSet {
            app: "ghost".into(),
            key: "k".into(),
            value: "v".into()
        }),
        Err(Error::AppNotFound("ghost".into()))
    );

    core.execute(Command::KvSet {
        app: "notes".into(),
        key: "theme".into(),
        value: "dark".into(),
    })
    .unwrap();
    assert_eq!(core.state().data["notes"]["theme"], "dark");
    assert!(core.replay_matches().unwrap());

    // Deleting a missing key errors; deleting a present key works.
    assert_eq!(
        core.execute(Command::KvDelete {
            app: "notes".into(),
            key: "ghost".into()
        }),
        Err(Error::KeyNotFound("notes".into(), "ghost".into()))
    );

    // Removing the app cascades: its data is gone from a fresh replay too.
    core.execute(Command::KvSet {
        app: "notes".into(),
        key: "lang".into(),
        value: "en".into(),
    })
    .unwrap();
    core.execute(Command::RemoveApp { id: "notes".into() })
        .unwrap();
    assert!(core.state().data.is_empty());
    assert!(replay(&log).unwrap().data.is_empty());
}

/// A deterministic stand-in for the network: every GET returns a canned body
/// derived from the url, so tests never touch the wire.
struct FakeHttp;

impl EffectRunner for FakeHttp {
    fn run(&self, effect: &Effect) -> Result<Vec<Event>> {
        match effect {
            Effect::HttpGet { app, url } => Ok(vec![Event::Fetched {
                app: app.clone(),
                url: url.clone(),
                status: 200,
                body: format!("body for {url}"),
            }]),
        }
    }
}

#[test]
fn fetch_effect_is_recorded_then_replays_without_the_runner() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open_with(&log, FakeHttp).unwrap();
    core.execute(add("notes", "Notes")).unwrap();
    core.execute(Command::Fetch {
        app: "notes".into(),
        url: "http://example.test/data".into(),
    })
    .unwrap();

    // The effect's result was recorded into State…
    let resp = &core.state().fetches["notes"]["http://example.test/data"];
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "body for http://example.test/data");

    // …as a Fetched event in the log…
    let events = terrane_core::read_log(&log).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Fetched { .. })));

    // …and a plain replay (no runner, no network) reproduces it exactly.
    let replayed = replay(&log).unwrap();
    assert_eq!(replayed.fetches, core.state().fetches);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn fetch_is_validated_purely_before_any_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    // A pure core (NoEffects): a valid Fetch reaches the runner and is refused…
    let mut core = Core::open(&log).unwrap();
    core.execute(add("notes", "Notes")).unwrap();
    assert!(matches!(
        core.execute(Command::Fetch {
            app: "notes".into(),
            url: "http://x/".into()
        }),
        Err(Error::InvalidInput(_))
    ));
    // …but a Fetch for a missing app is rejected in decide, before the runner.
    assert_eq!(
        core.execute(Command::Fetch {
            app: "ghost".into(),
            url: "http://x/".into()
        }),
        Err(Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn rejects_empty_fields() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(matches!(
        core.execute(add("", "x")),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.execute(add("x", "")),
        Err(Error::InvalidInput(_))
    ));
}
