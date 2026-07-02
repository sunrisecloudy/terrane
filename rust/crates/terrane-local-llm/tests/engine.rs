//! Tests for the local inference engine. The pure surface runs by default;
//! anything that loads real weights needs a GGUF at `TERRANE_LOCAL_MODEL_GGUF`
//! and is `#[ignore]`d.

use std::path::PathBuf;
use std::time::Duration;

use terrane_local_llm::{
    cached_llama, parse_json, resolve_runtime, server_status, stop_server, Constraint,
    GenerateRequest, GenerationConfig, LlamaCppBackend, LlmError, LocalLlm, MlxBackend, ModelFile,
};

fn gguf_from_env() -> Option<PathBuf> {
    match std::env::var("TERRANE_LOCAL_MODEL_GGUF") {
        Ok(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!("skipping: set TERRANE_LOCAL_MODEL_GGUF to a local .gguf file");
            None
        }
    }
}

#[test]
fn missing_model_file_fails_fast_with_a_typed_error() {
    let err = match LlamaCppBackend::load(&ModelFile {
        path: PathBuf::from("/nonexistent/model.gguf"),
        context_length: None,
        chat_template_override: None,
    }) {
        Err(err) => err,
        Ok(_) => panic!("loading a missing file should fail"),
    };
    assert!(matches!(err, LlmError::Load(_)));
    assert!(err.to_string().contains("/nonexistent/model.gguf"), "{err}");
}

#[test]
fn cached_llama_never_caches_load_failures() {
    let missing = ModelFile {
        path: PathBuf::from("/nonexistent/cached.gguf"),
        context_length: None,
        chat_template_override: None,
    };
    // Both attempts hit the loader (a cached error would mask a later fix).
    assert!(matches!(cached_llama(&missing), Err(LlmError::Load(_))));
    assert!(matches!(cached_llama(&missing), Err(LlmError::Load(_))));
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn cached_llama_reuses_the_loaded_engine_across_asks() {
    let Some(path) = gguf_from_env() else { return };
    let file = ModelFile {
        path,
        context_length: None,
        chat_template_override: None,
    };
    let started = std::time::Instant::now();
    let cold = cached_llama(&file).unwrap();
    let cold_load = started.elapsed();

    let started = std::time::Instant::now();
    let warm = cached_llama(&file).unwrap();
    let warm_load = started.elapsed();
    assert!(
        std::sync::Arc::ptr_eq(&cold, &warm),
        "same key returns the same engine"
    );
    assert!(
        warm_load < Duration::from_millis(10),
        "cache hit should be instant (cold {cold_load:?}, warm {warm_load:?})"
    );

    // The cached engine still generates (contexts are per-generate).
    let response = warm
        .lock()
        .unwrap()
        .generate(
            &GenerateRequest {
                prompt: "Reply with one word: hello".into(),
                system: None,
                history: Vec::new(),
                constraint: None,
                config: GenerationConfig {
                    max_tokens: 8,
                    temperature: 0.0,
                    timeout: Some(Duration::from_secs(120)),
                    ..GenerationConfig::default()
                },
            },
            &mut |_| {},
        )
        .unwrap();
    assert!(!response.text.trim().is_empty());
    eprintln!("cached_llama: cold load {cold_load:?}, cache hit {warm_load:?}");

    // Drop the cached engine before the test process exits — a live Metal
    // model during ggml's static destructors aborts the whole test binary.
    drop(cold);
    drop(warm);
    terrane_local_llm::clear_llama_cache();
}

#[test]
fn generation_config_defaults_are_sane() {
    let config = GenerationConfig::default();
    assert_eq!(config.max_tokens, 512);
    assert!(config.temperature > 0.0 && config.temperature <= 1.0);
    assert!(config.timeout.is_none());
}

#[test]
fn parse_json_extracts_typed_values_and_rejects_garbage() {
    #[derive(serde::Deserialize)]
    struct Answer {
        answer: String,
    }
    let parsed: Answer = parse_json(r#"  {"answer": "42"}  "#).unwrap();
    assert_eq!(parsed.answer, "42");
    assert!(parse_json::<Answer>("not json").is_err());
    assert!(parse_json::<Answer>(r#"{"other": 1}"#).is_err());
}

#[test]
#[ignore = "starts a real resident mlx server; needs the mlx-lm runtime; run with `cargo test -- --ignored`"]
fn resident_mlx_server_serves_warm_asks_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    if resolve_runtime(home).is_none() {
        eprintln!("skipping: no mlx runtime (run `terrane local-model setup mlx`)");
        return;
    }
    let mut backend = MlxBackend::load(home, "mlx-community/Qwen3.5-0.8B-MLX-4bit").unwrap();
    let request = GenerateRequest {
        prompt: "Reply with one word: hello".into(),
        system: None,
        history: Vec::new(),
        constraint: None,
        config: GenerationConfig {
            max_tokens: 16,
            temperature: 0.0,
            timeout: Some(Duration::from_secs(180)),
            ..GenerationConfig::default()
        },
    };

    // Cold: auto-starts the server (and pays the model load).
    let cold = backend.generate(&request, &mut |_| {}).unwrap();
    assert!(cold.ok(), "stopped by {:?}", cold.stop);
    let status = server_status(home);
    assert!(status.running, "server should be resident after an ask");
    assert!(status.socket.is_some() && status.pid.is_some());

    // Warm: the resident server answers fast.
    let started = std::time::Instant::now();
    let warm = backend.generate(&request, &mut |_| {}).unwrap();
    let elapsed = started.elapsed();
    assert!(warm.ok(), "stopped by {:?}", warm.stop);
    assert!(!warm.text.trim().is_empty());
    assert!(
        elapsed < Duration::from_secs(2),
        "warm ask should be sub-2s, took {elapsed:?}"
    );

    assert!(stop_server(home).unwrap(), "a resident server was stopped");
    assert!(!server_status(home).running);
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn generates_streamed_text_from_real_weights() {
    let Some(path) = gguf_from_env() else { return };
    let mut backend = LlamaCppBackend::load(&ModelFile {
        path,
        context_length: None,
        chat_template_override: None,
    })
    .unwrap();

    let mut streamed = String::new();
    let response = backend
        .generate(
            &GenerateRequest {
                prompt: "Reply with one short sentence: what is 2+2?".into(),
                system: None,
                history: Vec::new(),
                constraint: None,
                config: GenerationConfig {
                    max_tokens: 64,
                    timeout: Some(Duration::from_secs(120)),
                    ..GenerationConfig::default()
                },
            },
            &mut |piece| streamed.push_str(piece),
        )
        .unwrap();

    assert!(response.ok(), "stopped by {:?}", response.stop);
    assert!(!response.text.trim().is_empty());
    assert_eq!(streamed, response.text, "stream must equal recorded text");
    assert!(response.token_count > 0);
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn schema_constrained_generation_returns_matching_json() {
    let Some(path) = gguf_from_env() else { return };
    let mut backend = LlamaCppBackend::load(&ModelFile {
        path,
        context_length: None,
        chat_template_override: None,
    })
    .unwrap();

    #[derive(serde::Deserialize)]
    struct Answer {
        answer: String,
    }
    let schema =
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#;
    let response = backend
        .generate(
            &GenerateRequest {
                prompt: "What is the capital of France? Answer as JSON.".into(),
                system: None,
                history: Vec::new(),
                constraint: Some(Constraint::JsonSchema(schema.into())),
                config: GenerationConfig {
                    max_tokens: 128,
                    temperature: 0.0,
                    timeout: Some(Duration::from_secs(120)),
                    ..GenerationConfig::default()
                },
            },
            &mut |_| {},
        )
        .unwrap();

    assert!(response.ok(), "stopped by {:?}", response.stop);
    let parsed: Answer = parse_json(&response.text).unwrap();
    assert!(!parsed.answer.trim().is_empty());
}
