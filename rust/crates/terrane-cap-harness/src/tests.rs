use std::any::Any;

use terrane_cap_interface::{CapBus, QueryValue};

use super::*;

#[derive(Default)]
struct Store {
    harness: HarnessState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "harness" => Some(&self.harness),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "harness" => Some(&mut self.harness),
            _ => None,
        }
    }
}

struct AppBus;

impl CapBus for AppBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn prompts_fill_json_safe_app_values() {
    let prompt = app_bundle_prompt("app\"id", "Name", "make it");
    assert!(prompt.contains("make it"));
    assert!(prompt.contains(r#""app\"id""#));
    assert!(prompt.contains(r#""Name""#));
    assert!(prompt.contains("Use only Terrane resources that are available"));
    assert!(prompt.contains("backend top-level variables"));
    assert!(prompt.contains("render the returned backend state"));

    let run_prompt = run_js_prompt("demo", "write js");
    assert!(run_prompt.contains("write js"));
    assert!(run_prompt.contains(r#""demo""#));
}

#[test]
fn parse_run_js_output_extracts_non_empty_js() {
    assert_eq!(
        parse_run_js_output("before {\"js\":\"globalThis.done = true;\"} after").unwrap(),
        "globalThis.done = true;"
    );
    assert!(parse_run_js_output("{\"js\":\"   \"}")
        .unwrap_err()
        .to_string()
        .contains("generated js"));
}

#[test]
fn js_events_fold_run_lifecycle() {
    let cap = HarnessCapability;
    let mut store = Store::default();

    cap.fold(
        &mut store,
        &js_requested_event("run-1", "demo", "write js", "opencode").unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &js_generated_event("run-1", "globalThis.answer = 42;").unwrap(),
    )
    .unwrap();
    cap.fold(&mut store, &js_completed_event("run-1", "42").unwrap())
        .unwrap();

    let run = &store.harness.runs["run-1"];
    assert_eq!(run.harness, "opencode");
    assert_eq!(run.js.as_deref(), Some("globalThis.answer = 42;"));
    assert_eq!(run.output.as_deref(), Some("42"));

    cap.fold(&mut store, &js_failed_event("run-1", "boom").unwrap())
        .unwrap();
    let run = &store.harness.runs["run-1"];
    assert_eq!(run.output, None);
    assert_eq!(run.error.as_deref(), Some("boom"));
}

#[test]
fn run_js_decision_uses_supported_harnesses() {
    let store = Store::default();
    let bus = AppBus;

    assert_eq!(
        HarnessCapability
            .decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                "harness.run-js",
                &[
                    "--harness".into(),
                    "claude-code".into(),
                    "run_1".into(),
                    "demo".into(),
                    "write".into(),
                    "js".into(),
                ],
            )
            .unwrap(),
        Decision::Effect(Effect::RunHarnessJs {
            run_id: "run_1".into(),
            app_id: "demo".into(),
            harness: "claude-code".into(),
            prompt: "write js".into()
        })
    );
    assert!(HarnessCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "harness.run-js",
            &[
                "--harness".into(),
                "unknown".into(),
                "run_1".into(),
                "demo".into(),
                "write".into(),
            ],
        )
        .unwrap_err()
        .to_string()
        .contains("unsupported harness"));
}
