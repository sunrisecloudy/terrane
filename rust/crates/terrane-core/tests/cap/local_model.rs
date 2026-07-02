//! Engine tests for the `local-model` capability — registered specs plus the
//! recorded-inference effect, driven end to end with a stub runner.

use tempfile::tempdir;
use terrane_cap_local_model::responded_event;
use terrane_core::{fold_records_in_memory, Core, EffectRunner, Error, State};
use terrane_core::{Effect, EventRecord, Result};

use crate::helpers::req;

/// A deterministic stand-in for the llama.cpp edge engine: records a canned
/// response the way the real runner would.
struct StubLlm;

impl EffectRunner for StubLlm {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::LocalModelCall {
                app,
                model,
                prompt,
                schema,
                grammar,
            } => Ok(vec![responded_event(
                app,
                model,
                prompt,
                "stub response".to_string(),
                true,
                schema.is_some() || grammar.is_some(),
                3,
                12,
            )?]),
            other => Err(Error::InvalidInput(format!(
                "stub runner cannot perform {other:?}"
            ))),
        }
    }
}

#[test]
fn register_upserts_spec_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req(
        "local-model.register",
        &[
            "qwen",
            "llama_cpp",
            "/models/qwen.gguf",
            "--context",
            "8192",
            "--temp",
            "0.7",
        ],
    ))
    .unwrap();
    let spec = &core.state().local_model.specs["qwen"];
    assert_eq!(spec.local_path, "/models/qwen.gguf");
    assert_eq!(spec.format, "gguf");
    assert_eq!(spec.context_length, Some(8192));
    assert_eq!(spec.temperature_milli, Some(700));

    // Re-registering the same id overwrites the spec (one fact per id).
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen-q8.gguf"],
    ))
    .unwrap();
    assert_eq!(core.state().local_model.specs.len(), 1);
    assert_eq!(
        core.state().local_model.specs["qwen"].local_path,
        "/models/qwen-q8.gguf"
    );
    assert_eq!(core.state().local_model.specs["qwen"].context_length, None);

    assert!(core.replay_matches().unwrap());
    // A cold reopen rebuilds the same specs from the log alone.
    assert_eq!(
        Core::open(&log).unwrap().state().local_model.specs["qwen"].local_path,
        "/models/qwen-q8.gguf"
    );
}

#[test]
fn rm_removes_spec_and_rejects_unknown_ids() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();

    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();
    core.dispatch(req("local-model.rm", &["qwen"])).unwrap();
    assert!(core.state().local_model.specs.is_empty());

    assert!(matches!(
        core.dispatch(req("local-model.rm", &["qwen"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn ask_and_pull_validate_purely_before_any_effect() {
    let dir = tempdir().unwrap();
    // A pure core (NoEffects): a valid ask reaches the runner and is refused…
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();
    assert!(core
        .dispatch(req("local-model.ask", &["demo", "qwen", "hi"]))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));

    // …while bad requests are rejected in decide, before the runner.
    assert_eq!(
        core.dispatch(req("local-model.ask", &["ghost", "qwen", "hi"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    assert!(matches!(
        core.dispatch(req("local-model.ask", &["demo", "unregistered", "hi"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "local-model.ask",
            &[
                "demo",
                "qwen",
                "--schema",
                "{}",
                "--grammar",
                "root ::= \"x\"",
                "hi"
            ],
        )),
        Err(Error::InvalidInput(_))
    ));
    // Pull validation is pure too: a malformed repo never reaches the runner.
    assert!(matches!(
        core.dispatch(req("local-model.pull", &["qwen", "not-a-repo", "m.gguf"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn ask_records_turns_via_runner_and_cascades_on_app_removal() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubLlm).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();

    let events = core
        .dispatch(req("local-model.ask", &["demo", "qwen", "say", "hi"]))
        .unwrap();
    assert_eq!(events.len(), 1, "one recorded response per ask");
    let turns = &core.state().local_model.turns["demo"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].prompt, "say hi");
    assert_eq!(turns[0].response, "stub response");
    assert!(!turns[0].constrained);

    core.dispatch(req(
        "local-model.ask",
        &[
            "demo",
            "qwen",
            "--schema",
            r#"{"type":"object"}"#,
            "say",
            "hi",
        ],
    ))
    .unwrap();
    assert!(core.state().local_model.turns["demo"][1].constrained);
    assert!(core.replay_matches().unwrap());

    // Removing the app drops its transcript via broadcast fold but keeps the
    // machine-global spec.
    core.dispatch(req("app.remove", &["demo"])).unwrap();
    assert!(core.state().local_model.turns.is_empty());
    assert!(core.state().local_model.specs.contains_key("qwen"));
    assert!(core.replay_matches().unwrap());
    assert!(Core::open(&log)
        .unwrap()
        .state()
        .local_model
        .turns
        .is_empty());
}

#[test]
fn responded_event_folds_recorded_generation_without_inference() {
    let mut state = State::default();
    let records = vec![responded_event(
        "demo",
        "qwen",
        "say hi",
        "hello".to_string(),
        true,
        false,
        2,
        15,
    )
    .unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let turns = &state.local_model.turns["demo"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].model, "qwen");
    assert_eq!(turns[0].response, "hello");
    assert!(turns[0].ok);
    assert_eq!(turns[0].token_count, 2);
}
