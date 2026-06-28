//! Engine tests for the `model` capability — recorded agent calls.

use tempfile::tempdir;
use terrane_cap_model::responded_event;
use terrane_core::Error;
use terrane_core::{fold_records_in_memory, Core, State};

use crate::helpers::req;

#[test]
fn responded_event_folds_recorded_agent_response_without_agent() {
    let mut state = State::default();
    let records = vec![responded_event("asst", "claude", "say hi", "OK".to_string(), 0).unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let turns = &state.model.turns["asst"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].agent, "claude");
    assert_eq!(turns[0].prompt, "say hi");
    assert_eq!(turns[0].response, "OK");
    assert_eq!(turns[0].exit_code, 0);
}

#[test]
fn model_call_validates_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["asst", "Assistant"]))
        .unwrap();

    assert!(core
        .dispatch(req("model.ask", &["asst", "claude", "say", "hi"]))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));

    // An unknown agent is rejected purely, before any effect.
    assert!(matches!(
        core.dispatch(req("model.ask", &["asst", "bard", "hi"])),
        Err(Error::InvalidInput(_))
    ));
}
