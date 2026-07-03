//! Engine tests for the `agent` capability.

use tempfile::tempdir;
use terrane_core::Core;
use terrane_core::Error;

use crate::helpers::req;

#[test]
fn creates_with_defaults_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "agent.create",
        &["sara", "Sara", "--personality", "You beautify things"],
    ))
    .unwrap();

    let sara = &core.state().agent.agents["sara"];
    assert_eq!(sara.name, "Sara");
    assert_eq!(sara.personality, "You beautify things");
    // Harness and model fall back to the opencode defaults.
    assert_eq!(sara.harness, "opencode");
    assert_eq!(sara.model, "opencode-go/kimi-k2.7-code");
    assert!(sara.color.starts_with('#'));

    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn update_is_a_partial_change() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("agent.create", &["max", "Max"])).unwrap();
    // Change only the model; everything else stays put.
    core.dispatch(req("agent.update", &["max", "--model", "opencode/big-pickle"]))
        .unwrap();

    let max = &core.state().agent.agents["max"];
    assert_eq!(max.name, "Max");
    assert_eq!(max.model, "opencode/big-pickle");
    assert_eq!(max.harness, "opencode");

    // Rename via --name, and add an allowed cap.
    core.dispatch(req(
        "agent.update",
        &["max", "--name", "Maxine", "--cap", "kv"],
    ))
    .unwrap();
    let max = &core.state().agent.agents["max"];
    assert_eq!(max.name, "Maxine");
    assert_eq!(max.allowed_caps, vec!["kv".to_string()]);
    assert_eq!(max.model, "opencode/big-pickle");

    assert!(core.replay_matches().unwrap());
}

#[test]
fn removes_and_rejects_bad_input() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("agent.create", &["iris", "Iris"])).unwrap();
    // Duplicate id is rejected.
    assert!(matches!(
        core.dispatch(req("agent.create", &["iris", "Again"])),
        Err(Error::InvalidInput(_))
    ));
    // Unsafe id is rejected.
    assert!(matches!(
        core.dispatch(req("agent.create", &["bad/id", "Bad"])),
        Err(Error::InvalidInput(_))
    ));
    // Missing name is rejected.
    assert!(matches!(
        core.dispatch(req("agent.create", &["nameless"])),
        Err(Error::InvalidInput(_))
    ));
    // Updating / removing a ghost is rejected.
    assert!(matches!(
        core.dispatch(req("agent.update", &["ghost", "--name", "x"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("agent.remove", &["ghost"])),
        Err(Error::InvalidInput(_))
    ));

    core.dispatch(req("agent.remove", &["iris"])).unwrap();
    assert!(core.state().agent.agents.is_empty());
    assert!(core.replay_matches().unwrap());
}
