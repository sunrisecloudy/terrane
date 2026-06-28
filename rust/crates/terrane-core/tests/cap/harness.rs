//! Engine tests for harness app generation requests.

use tempfile::tempdir;
use terrane_cap_harness::{
    js_completed_event, js_failed_event, js_generated_event, js_requested_event,
    parse_run_js_output,
};
use terrane_core::{fold_records_in_memory, Core, NoEffects, State};

use crate::helpers::req;

#[test]
fn harness_js_events_fold_run_state() {
    let mut state = State::default();
    let records = vec![
        js_requested_event("run-1", "demo", "write files", "codex").unwrap(),
        js_generated_event(
            "run-1",
            r#"function handle(input){ctx.resource.kv.set("file:main.js","ok");return "wrote";}"#,
        )
        .unwrap(),
        js_completed_event("run-1", "wrote").unwrap(),
    ];

    fold_records_in_memory(&mut state, &records).unwrap();

    let run = &state.harness.runs["run-1"];
    assert_eq!(run.app_id, "demo");
    assert_eq!(run.harness, "codex");
    assert_eq!(run.output.as_deref(), Some("wrote"));
    assert!(run.js.as_deref().unwrap().contains("ctx.resource.kv.set"));
    assert!(run.error.is_none());
}

#[test]
fn harness_js_failed_event_records_error() {
    let mut state = State::default();

    fold_records_in_memory(
        &mut state,
        &[
            js_requested_event("run-1", "demo", "write files", "codex").unwrap(),
            js_failed_event("run-1", "syntax error").unwrap(),
        ],
    )
    .unwrap();

    let run = &state.harness.runs["run-1"];
    assert_eq!(run.error.as_deref(), Some("syntax error"));
    assert!(run.output.is_none());
}

#[test]
fn harness_generation_validates_request_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "harness.generate-app",
            &["bad/path", "demo", "Demo", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("unsafe"));

    assert!(core
        .dispatch(req("harness.generate-app", &["demo", "demo", "Demo", ""]))
        .unwrap_err()
        .to_string()
        .contains("prompt"));
}

#[test]
fn pure_core_rejects_harness_effect_without_runner() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "harness.generate-app",
            &["demo", "demo", "Demo", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}

#[test]
fn harness_generation_accepts_supported_harness_flag() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "harness.generate-app",
            &[
                "--harness",
                "claude-code",
                "demo",
                "demo",
                "Demo",
                "make app"
            ],
        ))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}

#[test]
fn harness_rejects_unsupported_harness() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "harness.generate-app",
            &["--harness", "other", "demo", "demo", "Demo", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("unsupported harness"));
}

#[test]
fn harness_run_js_validates_existing_app_and_prompt_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req("harness.run-js", &["run-1", "missing", "write app"]))
        .unwrap_err()
        .to_string()
        .contains("not found"));

    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    assert!(core
        .dispatch(req("harness.run-js", &["bad/path", "demo", "write app"]))
        .unwrap_err()
        .to_string()
        .contains("unsafe"));
    assert!(core
        .dispatch(req("harness.run-js", &["run-1", "demo", ""]))
        .unwrap_err()
        .to_string()
        .contains("prompt"));
    assert!(core
        .dispatch(req("harness.run-js", &["run-1", "demo", "write app"]))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
    assert!(core
        .dispatch(req(
            "harness.run-js",
            &["--harness", "opencode", "run-2", "demo", "write app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}

#[test]
fn parse_run_js_output_extracts_json_wrapped_javascript() {
    let js = parse_run_js_output(
        r#"
        done:
        {"js":"function handle(input){return \"ok\";}"}
        "#,
    )
    .unwrap();

    assert_eq!(js, r#"function handle(input){return "ok";}"#);
}
