use terrane_cap_interface::{
    arg, ensure_app_exists, required_tail, state_ref, CommandCtx, Decision, Effect, Error, Result,
};

use crate::events::{registered_event, removed_event};
use crate::types::{LocalModelSpec, LocalModelState, BACKENDS, RESERVED_BACKENDS};

/// The optional spec flags shared by `register` and `pull`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SpecOptions {
    pub context_length: Option<u32>,
    pub chat_template: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature_milli: Option<u32>,
}

/// `local-model.register <id> <backend> <path> [--context N] [--template T]
/// [--max-tokens N] [--temp F]` — record (or overwrite) a model spec pointing
/// at weights already on disk. The file itself is checked at the edge, at
/// inference time, keeping decide pure.
pub(crate) fn decide_register(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let id = model_id(args, 0)?;
    let backend = supported_backend(arg(args, 1, "backend")?)?;
    let local_path = arg(args, 2, "model path")?;
    if local_path.trim().is_empty() {
        return Err(Error::InvalidInput("model path must not be empty".into()));
    }
    let options = parse_spec_options(args, 3)?;
    let spec = LocalModelSpec {
        format: backend_format(&backend).to_string(),
        backend,
        local_path,
        context_length: options.context_length,
        chat_template: options.chat_template,
        max_tokens: options.max_tokens,
        temperature_milli: options.temperature_milli,
        source: None,
        size_bytes: None,
    };
    Ok(Decision::Commit(vec![registered_event(&id, &spec)?]))
}

/// `local-model.pull <id> <hf-repo> <file> [--context N] [--template T]
/// [--max-tokens N] [--temp F]` — download weights from Hugging Face at the
/// edge; the runner records `local-model.registered` with the resolved path.
pub(crate) fn decide_pull(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let id = model_id(args, 0)?;
    let repo = arg(args, 1, "hugging face repo (org/name)")?;
    let (org, name) = repo
        .split_once('/')
        .ok_or_else(|| Error::InvalidInput(format!("repo must be org/name, got {repo:?}")))?;
    if org.trim().is_empty() || name.trim().is_empty() || name.contains('/') {
        return Err(Error::InvalidInput(format!(
            "repo must be org/name, got {repo:?}"
        )));
    }
    let file = arg(args, 2, "model file name")?;
    if file.trim().is_empty() || file.contains('/') || file.contains('\\') || file.contains("..") {
        return Err(Error::InvalidInput(format!(
            "model file must be a plain file name, got {file:?}"
        )));
    }
    if !file.ends_with(".gguf") {
        return Err(Error::InvalidInput(format!(
            "only gguf files are supported for pull, got {file:?}"
        )));
    }
    let options = parse_spec_options(args, 3)?;
    Ok(Decision::Effect(Effect::LocalModelPull {
        id,
        repo,
        file,
        context_length: options.context_length,
        chat_template: options.chat_template,
        max_tokens: options.max_tokens,
        temperature_milli: options.temperature_milli,
    }))
}

/// `local-model.rm <id>` — unregister a spec. Weights on disk are untouched.
pub(crate) fn decide_rm(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let id = model_id(args, 0)?;
    let known = state_ref::<LocalModelState>(ctx.state, "local-model")?
        .specs
        .contains_key(&id);
    if !known {
        return Err(Error::InvalidInput(format!("unknown local model: {id}")));
    }
    Ok(Decision::Commit(vec![removed_event(&id)?]))
}

/// `local-model.ask <app> <model-id> [--schema <json>] [--grammar <gbnf>]
/// <prompt…>` — validate purely; inference runs at the edge.
pub(crate) fn decide_ask(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let model = arg(args, 1, "model id")?;
    ensure_app_exists(ctx.bus, &app)?;
    if !state_ref::<LocalModelState>(ctx.state, "local-model")?
        .specs
        .contains_key(&model)
    {
        return Err(Error::InvalidInput(format!(
            "unknown local model: {model}; register or pull it first"
        )));
    }

    let mut schema = None;
    let mut grammar = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--schema" => {
                schema = Some(json_object_schema(arg(args, i + 1, "--schema value")?)?);
                i += 2;
            }
            "--grammar" => {
                let value = arg(args, i + 1, "--grammar value")?;
                if value.trim().is_empty() {
                    return Err(Error::InvalidInput("--grammar must not be empty".into()));
                }
                grammar = Some(value);
                i += 2;
            }
            _ => break,
        }
    }
    if schema.is_some() && grammar.is_some() {
        return Err(Error::InvalidInput(
            "--schema and --grammar are mutually exclusive".into(),
        ));
    }
    let prompt = required_tail(args, i, "prompt")?;

    Ok(Decision::Effect(Effect::LocalModelCall {
        app,
        model,
        prompt,
        schema,
        grammar,
    }))
}

/// A model id is a plain token: it names a spec and a file stem at the edge.
fn model_id(args: &[String], index: usize) -> Result<String> {
    let id = arg(args, index, "model id")?;
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !valid {
        return Err(Error::InvalidInput(format!(
            "model id must be [A-Za-z0-9_.-]+, got {id:?}"
        )));
    }
    Ok(id)
}

fn supported_backend(raw: String) -> Result<String> {
    let backend = raw.trim();
    if BACKENDS.contains(&backend) {
        return Ok(backend.to_string());
    }
    if RESERVED_BACKENDS.contains(&backend) {
        return Err(Error::InvalidInput(format!(
            "backend {backend} is not supported yet (planned Apple-acceleration phase); use llama_cpp"
        )));
    }
    Err(Error::InvalidInput(format!(
        "unknown backend {backend:?}; expected one of {BACKENDS:?}"
    )))
}

fn backend_format(backend: &str) -> &'static str {
    match backend {
        "llama_cpp" => "gguf",
        // Reserved backends are refused before this is reached.
        _ => "unknown",
    }
}

pub(crate) fn parse_spec_options(args: &[String], from: usize) -> Result<SpecOptions> {
    let mut options = SpecOptions::default();
    let mut i = from;
    while i < args.len() {
        match args[i].as_str() {
            "--context" => {
                options.context_length = Some(positive_u32(args, i + 1, "--context")?);
                i += 2;
            }
            "--template" => {
                let value = arg(args, i + 1, "--template value")?;
                if value.trim().is_empty() {
                    return Err(Error::InvalidInput("--template must not be empty".into()));
                }
                options.chat_template = Some(value);
                i += 2;
            }
            "--max-tokens" => {
                options.max_tokens = Some(positive_u32(args, i + 1, "--max-tokens")?);
                i += 2;
            }
            "--temp" => {
                options.temperature_milli = Some(temperature_milli(arg(args, i + 1, "--temp")?)?);
                i += 2;
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown option {other:?}; expected --context, --template, --max-tokens, or --temp"
                )));
            }
        }
    }
    Ok(options)
}

fn positive_u32(args: &[String], index: usize, what: &str) -> Result<u32> {
    let value = arg(args, index, what)?;
    match value.parse::<u32>() {
        Ok(parsed) if parsed > 0 => Ok(parsed),
        _ => Err(Error::InvalidInput(format!(
            "{what} must be a positive integer, got {value:?}"
        ))),
    }
}

/// Parse a temperature like `0.7` into thousandths (`700`), so specs and
/// events stay integral (and the state slice stays `Eq`).
fn temperature_milli(raw: String) -> Result<u32> {
    let parsed = raw
        .parse::<f64>()
        .map_err(|_| Error::InvalidInput(format!("--temp must be a number, got {raw:?}")))?;
    if !(0.0..=2.0).contains(&parsed) {
        return Err(Error::InvalidInput(format!(
            "--temp must be between 0.0 and 2.0, got {raw}"
        )));
    }
    Ok((parsed * 1000.0).round() as u32)
}

/// Require a `--schema` value to be a JSON object before it reaches the edge.
fn json_object_schema(raw: String) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::InvalidInput(format!("--schema is not valid JSON: {e}")))?;
    if !value.is_object() {
        return Err(Error::InvalidInput("--schema must be a JSON object".into()));
    }
    Ok(raw)
}
