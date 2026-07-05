//! Engine tests for the `model` capability — recorded agent calls.

use tempfile::tempdir;
use terrane_cap_model::responded_event;
use terrane_core::Error;
use terrane_core::{fold_records_in_memory, Core, Effect, EffectRunner, EventRecord, Result, State};

use crate::helpers::req;

struct StubAgent;

impl EffectRunner for StubAgent {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::ModelCall {
                app,
                agent,
                prompt,
                image_parts,
            } => {
                if prompt.contains("with-image") {
                    assert_eq!(image_parts.len(), 1);
                    assert_eq!(image_parts[0].mime, "image/png");
                    assert_eq!(image_parts[0].size, 12);
                } else {
                    assert!(image_parts.is_empty());
                }
                Ok(vec![responded_event(
                    app,
                    agent,
                    prompt,
                    "stubbed".to_string(),
                    0,
                )?])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

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

#[test]
fn model_prompt_json_blob_names_normalize_to_content_refs_and_replay() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubAgent).unwrap();
    core.dispatch(req("app.add", &["asst", "Assistant"]))
        .unwrap();
    core.dispatch(req(
        "blob.link",
        &[
            "asst",
            "images/a.png",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "12",
            "image/png",
        ],
    ))
    .unwrap();

    let prompt =
        r#"{"parts":[{"text":"with-image"},{"blob":"images/a.png"}]}"#;
    core.dispatch(req("model.ask", &["asst", "codex", prompt]))
        .unwrap();

    let turn = &core.state().model.turns["asst"][0];
    assert!(turn.prompt.contains("\"hash\""));
    assert!(turn.prompt.contains("\"mime\":\"image/png\""));
    assert!(!turn.prompt.contains("base64"));
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().model,
        core.state().model
    );
}

#[test]
fn model_prompt_json_rejects_inline_bytes_and_image_limit_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), StubAgent).unwrap();
    core.dispatch(req("app.add", &["asst", "Assistant"]))
        .unwrap();

    let inline = r#"{"parts":[{"text":"x"},{"blob":{"hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","size":1,"mime":"image/png","base64":"AA=="}}]}"#;
    let err = core
        .dispatch(req("model.ask", &["asst", "codex", inline]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("inline base64"), "{err}");

    let too_large = r#"{"parts":[{"blob":{"hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","size":16777217,"mime":"image/png"}}]}"#;
    let err = core
        .dispatch(req("model.ask", &["asst", "codex", too_large]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("image part exceeds"), "{err}");
}

#[test]
fn model_per_app_spend_limit_blocks_after_recorded_turns() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), StubAgent).unwrap();
    core.dispatch(req("app.add", &["asst", "Assistant"]))
        .unwrap();
    for i in 0..terrane_cap_model::MAX_MODEL_CALLS_PER_APP {
        core.dispatch(req("model.ask", &["asst", "codex", &format!("turn {i}")]))
            .unwrap();
    }

    let err = core
        .dispatch(req("model.ask", &["asst", "codex", "one more"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("per-app recorded call limit"), "{err}");
}
