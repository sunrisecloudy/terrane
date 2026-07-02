//! Edge glue for the `local-model` capability: resolve the registered spec,
//! drive the `terrane-local-llm` engine (llama.cpp) exactly once per effect,
//! and hand back the capability's recorded events. Compiled with the
//! `local-llm` feature (on by default); a build without it refuses the
//! effects with a clear error instead.

use terrane_core::{Error, EventRecord, Result, State};

#[cfg(feature = "local-llm")]
const DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Progress lines while pulling weights, one every 16 MiB.
#[cfg(feature = "local-llm")]
const PROGRESS_STEP: u64 = 16 * 1024 * 1024;

/// Run one recorded local generation.
#[cfg(feature = "local-llm")]
pub(crate) fn call(
    app: &str,
    model: &str,
    prompt: &str,
    schema: Option<&str>,
    grammar: Option<&str>,
    state: &State,
) -> Result<Vec<EventRecord>> {
    use std::io::Write as _;

    use terrane_cap_local_model::responded_event;
    use terrane_local_llm::{
        Constraint, GenerateRequest, GenerationConfig, LlamaCppBackend, LocalLlm, ModelFile,
    };

    let spec = state
        .local_model
        .specs
        .get(model)
        .ok_or_else(|| Error::InvalidInput(format!("unknown local model: {model}")))?;
    if spec.backend != "llama_cpp" {
        return Err(Error::Runtime(format!(
            "local model backend {} has no edge engine yet",
            spec.backend
        )));
    }

    let mut backend = LlamaCppBackend::load(&ModelFile {
        path: std::path::PathBuf::from(&spec.local_path),
        context_length: spec.context_length,
        chat_template_override: spec.chat_template.clone(),
    })
    .map_err(|e| Error::Runtime(e.to_string()))?;

    let constraint = match (schema, grammar) {
        (Some(schema), _) => Some(Constraint::JsonSchema(schema.to_string())),
        (None, Some(grammar)) => Some(Constraint::Gbnf(grammar.to_string())),
        (None, None) => None,
    };
    let constrained = constraint.is_some();
    let request = GenerateRequest {
        prompt: prompt.to_string(),
        constraint,
        config: GenerationConfig {
            max_tokens: spec.max_tokens.unwrap_or(512),
            temperature: spec
                .temperature_milli
                .map_or(0.7, |milli| milli as f32 / 1000.0),
            timeout: Some(local_model_timeout()),
            ..GenerationConfig::default()
        },
    };

    // Stream tokens to stderr as they are sampled; stdout stays reserved for
    // the recorded outcome the CLI prints after commit.
    let mut streamed = false;
    let response = backend
        .generate(&request, &mut |piece| {
            streamed = true;
            eprint!("{piece}");
            let _ = std::io::stderr().flush();
        })
        .map_err(|e| Error::Runtime(e.to_string()))?;
    if streamed {
        eprintln!();
    }

    let ok = response.ok();
    let duration_ms = u64::try_from(response.duration.as_millis()).unwrap_or(u64::MAX);
    Ok(vec![responded_event(
        app,
        model,
        prompt,
        response.text,
        ok,
        constrained,
        response.token_count,
        duration_ms,
    )?])
}

/// Download weights from Hugging Face into `$TERRANE_HOME/models/` and record
/// the registered spec pointing at the finished file.
#[cfg(feature = "local-llm")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn pull(
    id: &str,
    repo: &str,
    file: &str,
    context_length: Option<u32>,
    chat_template: Option<String>,
    max_tokens: Option<u32>,
    temperature_milli: Option<u32>,
) -> Result<Vec<EventRecord>> {
    use terrane_cap_local_model::{registered_event, LocalModelSpec};

    let dest_dir = crate::home_dir().join("models");
    let mut last_reported = 0u64;
    let (path, size_bytes) =
        terrane_local_llm::download_model(repo, file, &dest_dir, &mut |written, total| {
            if written >= last_reported + PROGRESS_STEP {
                last_reported = written;
                let written_mib = written / (1024 * 1024);
                match total {
                    Some(total) => eprintln!(
                        "downloading {file}: {written_mib} / {} MiB",
                        total / (1024 * 1024)
                    ),
                    None => eprintln!("downloading {file}: {written_mib} MiB"),
                }
            }
        })
        .map_err(|e| Error::Storage(e.to_string()))?;

    let spec = LocalModelSpec {
        backend: "llama_cpp".to_string(),
        format: "gguf".to_string(),
        local_path: path.display().to_string(),
        context_length,
        chat_template,
        max_tokens,
        temperature_milli,
        source: Some(format!("hf:{repo}/{file}")),
        size_bytes: Some(size_bytes),
    };
    Ok(vec![registered_event(id, &spec)?])
}

#[cfg(feature = "local-llm")]
fn local_model_timeout() -> std::time::Duration {
    std::env::var("TERRANE_LOCAL_MODEL_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_millis(DEFAULT_TIMEOUT_MS))
}

#[cfg(not(feature = "local-llm"))]
pub(crate) fn call(
    _app: &str,
    _model: &str,
    _prompt: &str,
    _schema: Option<&str>,
    _grammar: Option<&str>,
    _state: &State,
) -> Result<Vec<EventRecord>> {
    Err(Error::Runtime(
        "this build has no local inference engine; rebuild terrane-host with --features local-llm"
            .into(),
    ))
}

#[cfg(not(feature = "local-llm"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn pull(
    _id: &str,
    _repo: &str,
    _file: &str,
    _context_length: Option<u32>,
    _chat_template: Option<String>,
    _max_tokens: Option<u32>,
    _temperature_milli: Option<u32>,
) -> Result<Vec<EventRecord>> {
    Err(Error::Runtime(
        "this build has no local inference engine; rebuild terrane-host with --features local-llm"
            .into(),
    ))
}
