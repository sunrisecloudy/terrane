//! Integration tests over the public `Capability` surface, with stub state and
//! bus. Engine-level behaviour (dispatch, replay) lives in
//! `terrane-core/tests/cap/local_model.rs`.

use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, EventRecord, QueryValue,
    StateStore,
};
use terrane_cap_local_model::{LocalModelCapability, LocalModelState};

struct Store {
    local_model: LocalModelState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        (namespace == "local-model").then_some(&self.local_model as &dyn Any)
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        (namespace == "local-model").then_some(&mut self.local_model as &mut dyn Any)
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            other => Err(Error::InvalidInput(format!("unexpected query: {other:?}"))),
        }
    }
}

#[derive(BorshSerialize)]
struct AppRemoved {
    id: String,
}

fn app_removed(id: &str) -> EventRecord {
    encode_event("app.removed", &AppRemoved { id: id.to_string() }).unwrap()
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

#[test]
fn register_ask_and_cascade_through_the_trait_surface() {
    let cap = LocalModelCapability;
    let mut store = Store {
        local_model: LocalModelState::default(),
    };
    let bus = Bus;

    // Register folds into a visible spec.
    let Decision::Commit(records) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "local-model.register",
            &strings(&["qwen", "llama_cpp", "/models/qwen.gguf", "--temp", "0.7"]),
        )
        .unwrap()
    else {
        panic!("register should commit");
    };
    for record in &records {
        cap.fold(&mut store, record).unwrap();
    }
    let spec = &store.local_model.specs["qwen"];
    assert_eq!(spec.backend, "llama_cpp");
    assert_eq!(spec.format, "gguf");
    assert_eq!(spec.local_path, "/models/qwen.gguf");
    assert_eq!(spec.temperature_milli, Some(700));

    // Ask is an effect carrying the validated request.
    let decision = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "local-model.ask",
            &strings(&["demo", "qwen", "say", "hi"]),
        )
        .unwrap();
    assert_eq!(
        decision,
        Decision::Effect(Effect::LocalModelCall {
            app: "demo".into(),
            model: "qwen".into(),
            prompt: "say hi".into(),
            schema: None,
            grammar: None,
        })
    );

    // A recorded response folds into the app's transcript…
    let responded = terrane_cap_local_model::responded_event(
        "demo",
        "qwen",
        "say hi",
        "hello".into(),
        true,
        false,
        2,
        15,
    )
    .unwrap();
    cap.fold(&mut store, &responded).unwrap();
    assert_eq!(store.local_model.turns["demo"].len(), 1);
    assert_eq!(store.local_model.turns["demo"][0].response, "hello");

    // …and app removal drops the transcript but keeps the machine-global spec.
    cap.fold(&mut store, &app_removed("demo")).unwrap();
    assert!(store.local_model.turns.is_empty());
    assert!(store.local_model.specs.contains_key("qwen"));

    // Foreign events fall through untouched.
    let foreign = encode_event("kv.set", &AppRemoved { id: "x".into() }).unwrap();
    cap.fold(&mut store, &foreign).unwrap();
    assert!(cap.describe(&foreign).is_none());

    // Rm folds the spec away.
    let Decision::Commit(records) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "local-model.rm",
            &strings(&["qwen"]),
        )
        .unwrap()
    else {
        panic!("rm should commit");
    };
    for record in &records {
        cap.fold(&mut store, record).unwrap();
    }
    assert!(store.local_model.specs.is_empty());
}

#[test]
fn describe_renders_own_events() {
    let cap = LocalModelCapability;

    let registered = terrane_cap_local_model::registered_event(
        "qwen",
        &terrane_cap_local_model::LocalModelSpec {
            backend: "llama_cpp".into(),
            format: "gguf".into(),
            local_path: "/models/qwen.gguf".into(),
            context_length: None,
            chat_template: None,
            max_tokens: None,
            temperature_milli: None,
            source: None,
            size_bytes: None,
        },
    )
    .unwrap();
    let line = cap.describe(&registered).unwrap();
    assert!(
        line.contains("qwen") && line.contains("llama_cpp"),
        "{line}"
    );

    let responded = terrane_cap_local_model::responded_event(
        "demo",
        "qwen",
        "say hi",
        "hello".into(),
        true,
        true,
        2,
        15,
    )
    .unwrap();
    let line = cap.describe(&responded).unwrap();
    assert!(
        line.contains("constrained") && line.contains("2 tokens"),
        "{line}"
    );

    let removed = terrane_cap_local_model::removed_event("qwen").unwrap();
    assert_eq!(cap.describe(&removed).unwrap(), "local-model.removed qwen");
}
