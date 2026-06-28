use std::any::Any;

use terrane_cap_harness::{
    app_bundle_prompt, js_completed_event, js_failed_event, js_generated_event, js_requested_event,
    parse_run_js_output, HarnessCapability, HarnessState,
};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};

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

struct AppBus {
    exists: bool,
}

impl CapBus for AppBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.exists)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn harness_capability_returns_generate_and_run_effects() {
    let cap = HarnessCapability;
    let store = Store::default();
    let bus = AppBus { exists: true };

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "harness.generate-app",
            &[
                "--harness".into(),
                "opencode".into(),
                "draft_1".into(),
                "calendar".into(),
                "Calendar".into(),
                "make".into(),
                "calendar".into(),
            ],
        )
        .unwrap(),
        Decision::Effect(Effect::GenerateAppWithHarness {
            draft_id: "draft_1".into(),
            app_id: "calendar".into(),
            name: "Calendar".into(),
            harness: "opencode".into(),
            prompt: "make calendar".into()
        })
    );
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "harness.run-js",
            &[
                "--harness".into(),
                "claude".into(),
                "run_1".into(),
                "calendar".into(),
                "write".into(),
                "js".into(),
            ],
        )
        .unwrap(),
        Decision::Effect(Effect::RunHarnessJs {
            run_id: "run_1".into(),
            app_id: "calendar".into(),
            harness: "claude".into(),
            prompt: "write js".into()
        })
    );
}

#[test]
fn harness_capability_folds_js_run_lifecycle_and_rejects_missing_apps() {
    let cap = HarnessCapability;
    let mut store = Store::default();

    cap.fold(
        &mut store,
        &js_requested_event("run_1", "calendar", "write js", "codex").unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &js_generated_event("run_1", "globalThis.ok = true;").unwrap(),
    )
    .unwrap();
    cap.fold(&mut store, &js_completed_event("run_1", "ok").unwrap())
        .unwrap();
    assert_eq!(
        store.harness.runs["run_1"].js.as_deref(),
        Some("globalThis.ok = true;")
    );
    assert_eq!(store.harness.runs["run_1"].output.as_deref(), Some("ok"));

    cap.fold(&mut store, &js_failed_event("run_1", "boom").unwrap())
        .unwrap();
    assert_eq!(store.harness.runs["run_1"].output, None);
    assert_eq!(store.harness.runs["run_1"].error.as_deref(), Some("boom"));

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: false },
            },
            "harness.run-js",
            &["run_2".into(), "missing".into(), "write".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("missing".into())
    );
}

#[test]
fn harness_prompts_and_output_parsing_are_public_contracts() {
    let prompt = app_bundle_prompt("calendar", "Calendar", "make app");
    assert!(prompt.contains("make app"));
    assert!(prompt.contains(r#""calendar""#));
    assert!(prompt.contains(r#""Calendar""#));

    assert_eq!(
        parse_run_js_output("header {\"js\":\"globalThis.answer = 42;\"} footer").unwrap(),
        "globalThis.answer = 42;"
    );
}
