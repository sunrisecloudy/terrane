//! Tests for the local inference engine. The pure surface runs by default;
//! anything that loads real weights needs a GGUF at `TERRANE_LOCAL_MODEL_GGUF`
//! and is `#[ignore]`d.

use std::path::PathBuf;
use std::time::Duration;

use terrane_local_llm::{
    parse_json, Constraint, GenerateRequest, GenerationConfig, LlamaCppBackend, LlmError, LocalLlm,
    ModelFile,
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
