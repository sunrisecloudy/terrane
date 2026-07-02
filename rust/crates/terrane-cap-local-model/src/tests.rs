//! Unit tests for decide-side parsing and validation. The trait surface is
//! covered in `tests/capability.rs`; engine behaviour in
//! `terrane-core/tests/cap/local_model.rs`.

use std::any::Any;

use terrane_cap_interface::{CapBus, CommandCtx, Decision, Effect, Error, QueryValue, StateStore};

use crate::commands::{decide_ask, decide_pull, decide_register, parse_spec_options, SpecOptions};
use crate::types::LocalModelState;

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

struct Bus {
    apps: Vec<String>,
}

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(
                args.first().is_some_and(|app| self.apps.contains(app)),
            )),
            other => Err(Error::InvalidInput(format!("unexpected query: {other:?}"))),
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn store_with(model: Option<&str>) -> Store {
    store_with_backend(model, "llama_cpp")
}

fn store_with_backend(model: Option<&str>, backend: &str) -> Store {
    let mut state = LocalModelState::default();
    if let Some(id) = model {
        state.specs.insert(
            id.to_string(),
            crate::LocalModelSpec {
                backend: backend.into(),
                format: if backend == "mlx" { "mlx" } else { "gguf" }.into(),
                local_path: "/models/m.gguf".into(),
                context_length: None,
                chat_template: None,
                max_tokens: None,
                temperature_milli: None,
                source: None,
                size_bytes: None,
            },
        );
        // Mirrors the fold: the first registered model becomes the default.
        state.default_model = Some(id.to_string());
    }
    Store { local_model: state }
}

#[test]
fn ask_rejects_gbnf_grammars_on_the_mlx_backend() {
    let store = store_with_backend(Some("qwen-mlx"), "mlx");
    let bus = Bus {
        apps: vec!["demo".into()],
    };
    let ctx = CommandCtx {
        state: &store,
        bus: &bus,
    };

    let err = decide_ask(
        ctx,
        &strings(&[
            "demo",
            "--model",
            "qwen-mlx",
            "--grammar",
            "root ::= \"x\"",
            "hi",
        ]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("llama_cpp-only"), "{err}");

    // A schema is fine on mlx — it lowers to typed output at the edge.
    let ok = decide_ask(
        ctx,
        &strings(&[
            "demo",
            "--model",
            "qwen-mlx",
            "--schema",
            r#"{"type":"object"}"#,
            "hi",
        ]),
    );
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn spec_options_parse_and_reject_unknown_flags() {
    let args = strings(&[
        "--context",
        "8192",
        "--temp",
        "0.7",
        "--max-tokens",
        "256",
        "--template",
        "chatml",
    ]);
    assert_eq!(
        parse_spec_options(&args, 0).unwrap(),
        SpecOptions {
            backend: None,
            context_length: Some(8192),
            chat_template: Some("chatml".into()),
            max_tokens: Some(256),
            temperature_milli: Some(700),
        }
    );

    for bad in [
        vec!["--verbose"],
        vec!["--context", "0"],
        vec!["--context", "many"],
        vec!["--temp", "2.5"],
        vec!["--temp", "warm"],
        vec!["--template", " "],
        vec!["--max-tokens"],
    ] {
        assert!(
            matches!(
                parse_spec_options(&strings(&bad), 0),
                Err(Error::InvalidInput(_))
            ),
            "expected rejection for {bad:?}"
        );
    }
}

#[test]
fn register_validates_id_backend_and_path() {
    let store = store_with(None);
    let bus = Bus { apps: Vec::new() };
    let ctx = || CommandCtx {
        state: &store,
        bus: &bus,
    };

    let ok = decide_register(ctx(), &strings(&["m1", "llama_cpp", "/models/m.gguf"])).unwrap();
    assert!(matches!(ok, Decision::Commit(records) if records.len() == 1));

    for bad in [
        vec!["bad id", "llama_cpp", "/m.gguf"],
        vec!["m1", "vllm", "/m.gguf"],
        vec!["m1", "llama_cpp", " "],
    ] {
        assert!(
            matches!(
                decide_register(ctx(), &strings(&bad)),
                Err(Error::InvalidInput(_))
            ),
            "expected rejection for {bad:?}"
        );
    }

    // The mlx backend registers with a repo id or local dir as its path.
    let ok = decide_register(
        ctx(),
        &strings(&["m2", "mlx", "mlx-community/Qwen3.5-0.8B-MLX-4bit"]),
    )
    .unwrap();
    assert!(matches!(ok, Decision::Commit(records) if records.len() == 1));
}

#[test]
fn pull_validates_repo_and_file_shape() {
    let store = store_with(None);
    let bus = Bus { apps: Vec::new() };
    let ctx = || CommandCtx {
        state: &store,
        bus: &bus,
    };

    let decision = decide_pull(
        ctx(),
        &strings(&[
            "qwen",
            "unsloth/Qwen3.5-0.8B-GGUF",
            "Qwen3.5-0.8B-Q4_K_M.gguf",
        ]),
    )
    .unwrap();
    let Decision::Effect(Effect::LocalModelPull { id, repo, file, .. }) = decision else {
        panic!("pull should be an effect");
    };
    assert_eq!(
        (id.as_str(), repo.as_str(), file.as_deref()),
        (
            "qwen",
            "unsloth/Qwen3.5-0.8B-GGUF",
            Some("Qwen3.5-0.8B-Q4_K_M.gguf")
        )
    );

    for bad in [
        vec!["qwen", "no-slash", "m.gguf"],
        vec!["qwen", "org/name", "nested/m.gguf"],
        vec!["qwen", "org/name", "../m.gguf"],
        vec!["qwen", "org/name", "m.safetensors"],
        vec!["only-an-id"],
        vec!["qwen", "org/name", "m.gguf", "extra-positional"],
    ] {
        assert!(
            matches!(
                decide_pull(ctx(), &strings(&bad)),
                Err(Error::InvalidInput(_))
            ),
            "expected rejection for {bad:?}"
        );
    }
}

#[test]
fn bare_pull_targets_the_recommended_model() {
    let store = store_with(None);
    let bus = Bus { apps: Vec::new() };
    let ctx = || CommandCtx {
        state: &store,
        bus: &bus,
    };

    let Decision::Effect(Effect::LocalModelPull { id, repo, file, .. }) =
        decide_pull(ctx(), &[]).unwrap()
    else {
        panic!("bare pull should be an effect");
    };
    assert_eq!(id, crate::RECOMMENDED_MODEL_ID);
    assert_eq!(repo, crate::RECOMMENDED_GGUF_REPO);
    assert_eq!(file.as_deref(), Some(crate::RECOMMENDED_GGUF_FILE));

    // A bare mlx pull targets the recommended MLX snapshot.
    let Decision::Effect(Effect::LocalModelPull {
        id,
        repo,
        backend,
        file,
        ..
    }) = decide_pull(ctx(), &strings(&["--backend", "mlx"])).unwrap()
    else {
        panic!("mlx pull should be an effect");
    };
    assert_eq!(id, crate::RECOMMENDED_MLX_MODEL_ID);
    assert_eq!(repo, crate::RECOMMENDED_MLX_REPO);
    assert_eq!(backend, "mlx");
    assert_eq!(file, None);

    // A file argument makes no sense for a repo snapshot.
    let err = decide_pull(
        ctx(),
        &strings(&["m", "org/name", "x.gguf", "--backend", "mlx"]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("drop the file"), "{err}");
}

#[test]
fn ask_resolves_models_parses_constraints_and_rejects_conflicts() {
    let store = store_with(Some("qwen"));
    let bus = Bus {
        apps: vec!["demo".into()],
    };
    let ctx = || CommandCtx {
        state: &store,
        bus: &bus,
    };
    let schema = r#"{"type":"object"}"#;

    // Explicit --model wins.
    let decision = decide_ask(
        ctx(),
        &strings(&["demo", "--model", "qwen", "--schema", schema, "hi"]),
    )
    .unwrap();
    let Decision::Effect(Effect::LocalModelCall {
        app,
        model,
        prompt,
        system,
        history,
        schema: parsed_schema,
        grammar,
    }) = decision
    else {
        panic!("ask should be an effect");
    };
    assert_eq!(
        (app.as_str(), model.as_str(), prompt.as_str()),
        ("demo", "qwen", "hi")
    );
    assert_eq!(parsed_schema.as_deref(), Some(schema));
    assert_eq!(grammar, None);
    assert_eq!(system, None);
    assert!(history.is_empty());

    // No --model: the default model answers.
    let decision = decide_ask(ctx(), &strings(&["demo", "hello", "there"])).unwrap();
    let Decision::Effect(Effect::LocalModelCall { model, prompt, .. }) = decision else {
        panic!("ask should be an effect");
    };
    assert_eq!(model, "qwen");
    assert_eq!(prompt, "hello there");

    // A prompt that merely starts with a dash-word is still a prompt.
    let decision = decide_ask(ctx(), &strings(&["demo", "--not-a-flag", "hi"])).unwrap();
    let Decision::Effect(Effect::LocalModelCall { prompt, .. }) = decision else {
        panic!("ask should be an effect");
    };
    assert_eq!(prompt, "--not-a-flag hi");

    for bad in [
        vec!["ghost", "hi"],
        vec!["demo", "--model", "unregistered", "hi"],
        vec!["demo", "--schema", "[1,2]", "hi"],
        vec!["demo", "--schema", "not json", "hi"],
        vec![
            "demo",
            "--schema",
            schema,
            "--grammar",
            "root ::= \"x\"",
            "hi",
        ],
        vec!["demo", "--schema", schema],
        vec!["demo"],
    ] {
        assert!(
            decide_ask(ctx(), &strings(&bad)).is_err(),
            "expected rejection for {bad:?}"
        );
    }
}

#[test]
fn continue_builds_history_from_matching_ok_turns() {
    use crate::commands::{conversation_history, CONTINUE_TURN_LIMIT};
    use crate::LocalModelTurn;

    let mut store = store_with(Some("qwen"));
    let turn = |model: &str, prompt: &str, ok: bool| LocalModelTurn {
        model: model.into(),
        prompt: prompt.into(),
        system: None,
        continued: false,
        response: format!("re: {prompt}"),
        ok,
        constraint: None,
        token_count: 1,
        duration_ms: 1,
    };
    store.local_model.turns.insert(
        "demo".into(),
        vec![
            turn("qwen", "q1", true),
            turn("other-model", "skipped", true),
            turn("qwen", "failed", false),
            turn("qwen", "q2", true),
        ],
    );

    let history = conversation_history(&store.local_model, "demo", "qwen");
    assert_eq!(
        history,
        vec![
            ("q1".to_string(), "re: q1".to_string()),
            ("q2".to_string(), "re: q2".to_string()),
        ]
    );
    assert!(conversation_history(&store.local_model, "unknown-app", "qwen").is_empty());

    // Long transcripts are capped at the most recent exchanges.
    let many: Vec<_> = (0..20)
        .map(|i| turn("qwen", &format!("q{i}"), true))
        .collect();
    store.local_model.turns.insert("busy".into(), many);
    let history = conversation_history(&store.local_model, "busy", "qwen");
    assert_eq!(history.len(), CONTINUE_TURN_LIMIT);
    assert_eq!(history.last().unwrap().0, "q19");
    assert_eq!(history.first().unwrap().0, "q12");
}

#[test]
fn ask_without_any_model_explains_the_zero_config_path() {
    let store = store_with(None);
    let bus = Bus {
        apps: vec!["demo".into()],
    };
    let err = decide_ask(
        CommandCtx {
            state: &store,
            bus: &bus,
        },
        &strings(&["demo", "hi"]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("local-model pull"), "{err}");

    // Models registered but no default: the error lists the candidates.
    let mut store = store_with(Some("qwen"));
    store.local_model.default_model = None;
    let err = decide_ask(
        CommandCtx {
            state: &store,
            bus: &bus,
        },
        &strings(&["demo", "hi"]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("qwen"), "{err}");
}
