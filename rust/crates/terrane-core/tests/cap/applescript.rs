//! Engine tests for the `applescript` capability.

use tempfile::tempdir;
use terrane_cap_applescript::ran_event;
use terrane_core::{fold_records_in_memory, Core, Effect, EffectRunner, Error, EventRecord, Result, State};

use crate::helpers::req;

struct StubAppleScript;

impl EffectRunner for StubAppleScript {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::AppleScriptRun { app, script } => Ok(vec![ran_event(
                app, script, true, "4", "", 0, 5,
            )?]),
            other => Err(Error::InvalidInput(format!(
                "stub runner cannot perform {other:?}"
            ))),
        }
    }
}

#[test]
fn ran_event_folds_recorded_run_without_spawn() {
    let mut state = State::default();
    let records = vec![ran_event("mac", "return 2 + 2", true, "4", "", 0, 9).unwrap()];
    fold_records_in_memory(&mut state, &records).unwrap();
    assert_eq!(state.applescript.runs["mac"][0].output, "4");
}

#[test]
fn applescript_run_is_validated_before_any_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["mac", "Mac"])).unwrap();
    assert!(matches!(
        core.dispatch(req("applescript.run", &["mac", "return 1"])),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        core.dispatch(req("applescript.run", &["ghost", "return 1"])),
        Err(Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn applescript_run_replays_identically_with_stub_runner() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubAppleScript).unwrap();
    core.dispatch(req("app.add", &["mac", "Mac"])).unwrap();
    core.dispatch(req("applescript.run", &["mac", "return 2 + 2"]))
        .unwrap();
    assert!(core.replay_matches().unwrap());
}