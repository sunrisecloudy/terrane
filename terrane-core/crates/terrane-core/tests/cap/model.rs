//! Engine tests for the `model` capability — recorded agent calls.

use tempfile::tempdir;
use terrane_core::Core;
use terrane_domain::Error;

use crate::helpers::{req, FakeEdge};

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
