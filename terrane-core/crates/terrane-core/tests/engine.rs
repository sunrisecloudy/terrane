//! Integration tests for the terrane-core engine, driven through its public
//! surface (`Core::dispatch` with `Request`s) — kept out of the implementation
//! so the engine reads as one thing and its proofs as another.

use tempfile::tempdir;
use terrane_core::cap::model::responded_event;
use terrane_core::cap::net::fetched_event;
use terrane_core::{Core, Effect, EffectRunner};
use terrane_domain::{Error, EventRecord, Request, Result};

fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}

#[test]
fn dispatches_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("app.add", &["tasks", "Task", "Workbench"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();

    assert!(core.replay_matches().unwrap());
    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.state().app.apps.contains_key("tasks"));

    // A brand-new Core opened on the same log rebuilds the same world.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn source_round_trips_through_the_log() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes", "--source", "apps/notes"]))
        .unwrap();
    let reopened = Core::open(&log).unwrap();
    assert_eq!(
        reopened.state().app.apps["notes"].source.as_deref(),
        Some("apps/notes")
    );
}

#[test]
fn rejects_duplicate_missing_and_unknown() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert_eq!(
        core.dispatch(req("app.add", &["notes", "Again"])),
        Err(Error::AppExists("notes".into()))
    );
    assert_eq!(
        core.dispatch(req("app.remove", &["ghost"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    // Unknown namespace and unknown verb are both rejected.
    assert!(matches!(
        core.dispatch(req("bogus.thing", &[])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.frobnicate", &["x"])),
        Err(Error::InvalidInput(_))
    ));

    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn kv_records_and_cascades_via_broadcast_fold() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    // Writing to an app that doesn't exist is rejected.
    assert_eq!(
        core.dispatch(req("kv.set", &["ghost", "k", "v"])),
        Err(Error::AppNotFound("ghost".into()))
    );

    core.dispatch(req("kv.set", &["notes", "theme", "dark"]))
        .unwrap();
    assert_eq!(core.state().kv.data["notes"]["theme"], "dark");
    assert!(core.replay_matches().unwrap());

    // Deleting a missing key errors.
    assert_eq!(
        core.dispatch(req("kv.rm", &["notes", "ghost"])),
        Err(Error::KeyNotFound("notes".into(), "ghost".into()))
    );

    // Removing the app cascades to its data — the kv capability reacts to the
    // app.removed event via broadcast fold, with no app→kv coupling.
    core.dispatch(req("kv.set", &["notes", "lang", "en"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(core.state().kv.data.is_empty());
    assert!(Core::open(&log).unwrap().state().kv.data.is_empty());
}

#[test]
fn rejects_empty_fields() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(matches!(
        core.dispatch(req("app.add", &["", "x"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.add", &["x"])),
        Err(Error::InvalidInput(_))
    ));
}

/// A deterministic stand-in for the edge: canned responses for every effect, so
/// tests never touch the network or spawn a real agent.
struct FakeEdge;

impl EffectRunner for FakeEdge {
    fn run(&self, effect: &Effect) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                Ok(vec![fetched_event(app, url, 200, format!("body for {url}"))?])
            }
            Effect::ModelCall { app, agent, prompt } => Ok(vec![responded_event(
                app,
                agent,
                prompt,
                format!("{agent} says: {prompt}"),
                0,
            )?]),
        }
    }
}

#[test]
fn fetch_effect_is_recorded_then_replays_without_the_runner() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open_with(&log, FakeEdge).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("net.fetch", &["notes", "http://example.test/data"]))
        .unwrap();

    let resp = &core.state().net.fetches["notes"]["http://example.test/data"];
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "body for http://example.test/data");

    let records = terrane_core::read_log(&log).unwrap();
    assert!(records.iter().any(|r| r.kind == "net.fetched"));

    // Reopening with NO runner folds the log and reproduces the fetch — proof
    // that replay reads the body from the log, not the network.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().net.fetches, core.state().net.fetches);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn model_call_is_recorded_then_replays_without_the_agent() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open_with(&log, FakeEdge).unwrap();
    core.dispatch(req("app.add", &["asst", "Assistant"])).unwrap();
    core.dispatch(req("model.ask", &["asst", "claude", "say", "hi"]))
        .unwrap();

    let turns = &core.state().model.turns["asst"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].agent, "claude");
    assert_eq!(turns[0].prompt, "say hi");
    assert_eq!(turns[0].response, "claude says: say hi");
    assert_eq!(turns[0].exit_code, 0);

    // Reopening with NO runner folds the log and reproduces the transcript —
    // proof that replay reads the response from the log, not the agent.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().model.turns, core.state().model.turns);
    assert!(core.replay_matches().unwrap());

    // An unknown agent is rejected purely, before any effect.
    assert!(matches!(
        core.dispatch(req("model.ask", &["asst", "bard", "hi"])),
        Err(Error::InvalidInput(_))
    ));
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
