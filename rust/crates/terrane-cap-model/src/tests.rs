use std::any::Any;

use terrane_cap_interface::{CapBus, QueryValue};

use super::*;

#[derive(Default)]
struct Store {
    model: ModelState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "model" => Some(&self.model),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "model" => Some(&mut self.model),
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
fn responded_event_describes_and_folds_turn() {
    let mut store = Store::default();
    let cap = ModelCapability;
    let record = responded_event("demo", "codex", "say hi", "hi".into(), 0).unwrap();

    let description = cap.describe(&record).unwrap();
    assert!(description.contains("model.responded demo via codex"));
    assert!(description.contains("exit 0"));
    cap.fold(&mut store, &record).unwrap();
    assert_eq!(store.model.turns["demo"][0].response, "hi");
}

#[test]
fn ask_decision_rejects_unknown_agents_and_empty_prompts() {
    let store = Store::default();
    let bus = AppBus;

    assert_eq!(
        ModelCapability
            .decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                "model.ask",
                &["demo".into(), "codex".into(), "build".into(), "this".into()],
            )
            .unwrap(),
        Decision::Effect(Effect::ModelCall {
            app: "demo".into(),
            agent: "codex".into(),
            prompt: "build this".into()
        })
    );
    assert!(ModelCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "model.ask",
            &["demo".into(), "other".into(), "prompt".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("unknown agent"));
    assert_eq!(
        ModelCapability
            .decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                "model.ask",
                &["demo".into(), "codex".into()],
            )
            .unwrap_err(),
        Error::InvalidInput("prompt must not be empty".into())
    );
}
