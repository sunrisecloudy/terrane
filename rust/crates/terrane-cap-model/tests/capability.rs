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
            prompt: "make counter".into(),
            image_parts: Vec::new(),
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

#[test]
fn model_doc_covers_recorded_model_effects_and_app_cleanup() {
    let doc = ModelCapability.doc(false);

    assert_eq!(doc.namespace, "model");
    assert_eq!(doc.manifest.commands, vec!["model.ask".to_string()]);
    assert_eq!(doc.manifest.events, vec!["model.responded".to_string()]);
    assert_eq!(doc.manifest.subscriptions, vec!["app.removed".to_string()]);
    assert!(doc.manifest.queries.is_empty());
    assert_eq!(doc.manifest.resource_methods[0].name, "ask");
    assert_eq!(doc.resources[0].namespace, "model");
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("ModelCall") || constraint.contains("agent CLI")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("never by replay")));
    assert!(doc
        .constraints
        .iter()
        .any(|constraint| constraint.contains("Folding app.removed removes")));
    assert!(doc
        .limits
        .iter()
        .any(|limit| limit.name == "supportedAgents" && limit.value.contains("codex")));
    assert!(doc.internal.is_empty());

    assert!(ModelCapability
        .doc(true)
        .internal
        .iter()
        .any(|note| note.title.contains("Replay boundary")));
}
