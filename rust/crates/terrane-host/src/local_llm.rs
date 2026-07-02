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
#[allow(clippy::too_many_arguments)]
pub(crate) fn call(
    app: &str,
    model: &str,
    prompt: &str,
    system: Option<&str>,
    history: &[(String, String)],
    schema: Option<&str>,
    grammar: Option<&str>,
    state: &State,
) -> Result<Vec<EventRecord>> {
    use std::io::Write as _;

    use terrane_cap_local_model::{responded_event, RespondedRecord};
    use terrane_local_llm::{
        cached_llama, Constraint, GenerateRequest, GenerationConfig, LocalLlm, MlxBackend,
        ModelFile,
    };

    let spec = state
        .local_model
        .specs
        .get(model)
        .ok_or_else(|| Error::InvalidInput(format!("unknown local model: {model}")))?;

    let constraint = match (schema, grammar) {
        (Some(schema), _) => Some(Constraint::JsonSchema(schema.to_string())),
        (None, Some(grammar)) => Some(Constraint::Gbnf(grammar.to_string())),
        (None, None) => None,
    };
    let request = GenerateRequest {
        prompt: prompt.to_string(),
        system: system.map(str::to_string),
        history: history.to_vec(),
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
    let mut on_token = |piece: &str| {
        streamed = true;
        eprint!("{piece}");
        let _ = std::io::stderr().flush();
    };
    let response = match spec.backend.as_str() {
        "llama_cpp" => {
            // Cached process-globally: long-lived hosts pay the GGUF load once.
            let engine = cached_llama(&ModelFile {
                path: std::path::PathBuf::from(&spec.local_path),
                context_length: spec.context_length,
                chat_template_override: spec.chat_template.clone(),
            })
            .map_err(|e| Error::Runtime(e.to_string()))?;
            let mut engine = engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            engine.generate(&request, &mut on_token)
        }
        "mlx" => MlxBackend::load(&crate::home_dir(), &spec.local_path)
            .map_err(|e| Error::Runtime(e.to_string()))?
            .with_draft(spec.draft_model.clone())
            .generate(&request, &mut on_token),
        other => {
            return Err(Error::Runtime(format!(
                "local model backend {other} has no edge engine"
            )))
        }
    }
    .map_err(|e| Error::Runtime(e.to_string()))?;
    if streamed {
        eprintln!();
    }

    let ok = response.ok();
    let duration_ms = u64::try_from(response.duration.as_millis()).unwrap_or(u64::MAX);
    Ok(vec![responded_event(&RespondedRecord {
        app: app.to_string(),
        model: model.to_string(),
        prompt: prompt.to_string(),
        system: system.map(str::to_string),
        continued: !history.is_empty(),
        response: response.text,
        ok,
        constraint: response.constraint,
        token_count: response.token_count,
        duration_ms,
    })?])
}

/// Download weights from Hugging Face and record the registered spec: gguf
/// files land in `$TERRANE_HOME/models/`; mlx repos snapshot into the HF
/// cache (pre-warming what the worker would otherwise fetch on first ask).
#[cfg(feature = "local-llm")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn pull(
    id: &str,
    repo: &str,
    backend: &str,
    file: Option<&str>,
    context_length: Option<u32>,
    chat_template: Option<String>,
    max_tokens: Option<u32>,
    temperature_milli: Option<u32>,
    draft_model: Option<String>,
) -> Result<Vec<EventRecord>> {
    use terrane_cap_local_model::{registered_event, LocalModelSpec};

    let spec = match backend {
        "mlx" => {
            let (_snapshot, size_bytes) =
                terrane_local_llm::snapshot_mlx_repo(&crate::home_dir(), repo, &mut |line| {
                    eprintln!("{line}");
                })
                .map_err(|e| Error::Storage(e.to_string()))?;
            LocalModelSpec {
                backend: "mlx".to_string(),
                format: "mlx".to_string(),
                // The repo id stays the reference; the snapshot pre-warmed it.
                local_path: repo.to_string(),
                context_length,
                chat_template,
                max_tokens,
                temperature_milli,
                source: Some(format!("hf:{repo}")),
                size_bytes: Some(size_bytes),
                draft_model,
            }
        }
        _ => {
            let file = file.ok_or_else(|| {
                Error::InvalidInput("gguf pull needs a file name inside the repo".into())
            })?;
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
            LocalModelSpec {
                backend: "llama_cpp".to_string(),
                format: "gguf".to_string(),
                local_path: path.display().to_string(),
                context_length,
                chat_template,
                max_tokens,
                temperature_milli,
                source: Some(format!("hf:{repo}/{file}")),
                size_bytes: Some(size_bytes),
                draft_model: None,
            }
        }
    };
    Ok(vec![registered_event(id, &spec)?])
}

/// Drop the in-process engine cache. Hosts MUST call this before a normal
/// exit: a cached llama.cpp model still holding Metal buffers when ggml's
/// static destructors run aborts the process (residency-set assert).
#[cfg(feature = "local-llm")]
pub(crate) fn shutdown() {
    terrane_local_llm::clear_llama_cache();
}

#[cfg(not(feature = "local-llm"))]
pub(crate) fn shutdown() {}

/// `terrane local-model setup mlx` — provision the MLX runtime (uv-pinned,
/// self-contained under `$TERRANE_HOME/engines/`); progress lines go to
/// stderr. Host verb, not a capability command: nothing is recorded.
#[cfg(feature = "local-llm")]
pub(crate) fn setup_mlx(home: &std::path::Path) -> Result<String> {
    let report = terrane_local_llm::setup_mlx(home, &mut |line| {
        eprintln!("{line}");
    })
    .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(report.summary)
}

/// `terrane local-model server status` — resident-server state as a JSON
/// object (stable surface for the CLI and the C ABI).
#[cfg(feature = "local-llm")]
pub(crate) fn mlx_server_status_json(home: &std::path::Path) -> String {
    let status = terrane_local_llm::server_status(home);
    let runtime = terrane_local_llm::resolve_runtime(home);
    serde_json::json!({
        "running": status.running,
        "pid": status.pid,
        "socket": status.socket,
        "idleSecs": status.idle_secs,
        "models": status.models,
        "runtimeAvailable": runtime.is_some(),
        "runtimeSource": runtime.map(|r| r.source.describe()),
    })
    .to_string()
}

/// `terrane local-model server stop` — kill the resident server if any.
#[cfg(feature = "local-llm")]
pub(crate) fn mlx_server_stop(home: &std::path::Path) -> Result<String> {
    let stopped =
        terrane_local_llm::stop_server(home).map_err(|e| Error::Storage(e.to_string()))?;
    Ok(if stopped {
        "mlx server stopped".to_string()
    } else {
        "no resident mlx server".to_string()
    })
}

#[cfg(not(feature = "local-llm"))]
pub(crate) fn setup_mlx(_home: &std::path::Path) -> Result<String> {
    Err(no_local_llm())
}

#[cfg(not(feature = "local-llm"))]
pub(crate) fn mlx_server_status_json(_home: &std::path::Path) -> String {
    concat!(
        r#"{"running":false,"pid":null,"port":null,"idleSecs":null,"models":[],"#,
        r#""runtimeAvailable":false,"runtimeSource":null}"#
    )
    .to_string()
}

#[cfg(not(feature = "local-llm"))]
pub(crate) fn mlx_server_stop(_home: &std::path::Path) -> Result<String> {
    Err(no_local_llm())
}

#[cfg(not(feature = "local-llm"))]
fn no_local_llm() -> Error {
    Error::Runtime(
        "this build has no local inference engine; rebuild terrane-host with --features local-llm"
            .into(),
    )
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
    _backend: &str,
    _file: Option<&str>,
    _context_length: Option<u32>,
    _chat_template: Option<String>,
    _max_tokens: Option<u32>,
    _temperature_milli: Option<u32>,
    _draft_model: Option<String>,
) -> Result<Vec<EventRecord>> {
    Err(Error::Runtime(
        "this build has no local inference engine; rebuild terrane-host with --features local-llm"
            .into(),
    ))
}
