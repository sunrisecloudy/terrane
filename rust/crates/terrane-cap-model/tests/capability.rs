use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};
use terrane_cap_model::{responded_event, ModelCapability, ModelState, ModelTurn};

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

#[derive(BorshSerialize)]
struct Removed {
    id: String,
}

#[test]
fn model_capability_returns_effect_and_folds_recorded_turn() {
    let cap = ModelCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "model.ask",
            &[
                "demo".into(),
                "codex".into(),
                "make".into(),
                "counter".into()
            ],
        )
        .unwrap(),
        Decision::Effect(Effect::ModelCall {
            app: "demo".into(),
            agent: "codex".into(),
            prompt: "make counter".into()
        })
    );

    cap.fold(
        &mut store,
        &responded_event("demo", "codex", "make counter", "done".into(), 0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.model.turns["demo"][0],
        ModelTurn {
            agent: "codex".into(),
            prompt: "make counter".into(),
            response: "done".into(),
            exit_code: 0
        }
    );
}

#[test]
fn model_capability_rejects_invalid_requests_and_cleans_removed_apps() {
    let cap = ModelCapability;
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: false },
            },
            "model.ask",
            &["demo".into(), "codex".into(), "prompt".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("demo".into())
    );
    assert!(cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: true },
            },
            "model.ask",
            &["demo".into(), "other".into(), "prompt".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("unknown agent"));

    cap.fold(
        &mut store,
        &responded_event("demo", "codex", "prompt", "response".into(), 0).unwrap(),
    )
    .unwrap();
    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(store.model.turns.is_empty());
}
