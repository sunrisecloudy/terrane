use terrane_cap_interface::{
    arg, ensure_app_exists, required_tail, state_ref, CommandCtx, Decision, Effect, Error, Result,
};

use crate::events::{default_set_event, registered_event, removed_event};
use crate::types::{
    LocalModelSpec, LocalModelState, BACKENDS, RECOMMENDED_GGUF_FILE, RECOMMENDED_GGUF_REPO,
    RECOMMENDED_MODEL_ID,
};

/// The optional spec flags shared by `register` and `pull`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SpecOptions {
    pub backend: Option<String>,
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
    if options.backend.is_some() {
        return Err(Error::InvalidInput(
            "--backend is a pull option; register takes the backend positionally".into(),
        ));
    }
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

/// `local-model.default <id>` — make a registered model the one `ask` uses
/// when no `--model` is given.
pub(crate) fn decide_default(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let id = model_id(args, 0)?;
    let known = state_ref::<LocalModelState>(ctx.state, "local-model")?
        .specs
        .contains_key(&id);
    if !known {
        return Err(Error::InvalidInput(format!("unknown local model: {id}")));
    }
    Ok(Decision::Commit(vec![default_set_event(&id)?]))
}

/// `local-model.pull [<id> <hf-repo> [<file>]] [--backend gguf|mlx]
/// [--context N] [--template T] [--max-tokens N] [--temp F]` — download
/// weights from Hugging Face at the edge; the runner records
/// `local-model.registered` with the resolved path. A bare `pull` fetches the
/// recommended model (Qwen3.5-0.8B).
pub(crate) fn decide_pull(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    // Split positionals (id, repo, optional file) from the flag tail.
    let flags_from = args
        .iter()
        .position(|a| a.starts_with("--"))
        .unwrap_or(args.len());
    let positional = &args[..flags_from];
    let options = parse_spec_options(args, flags_from)?;
    let backend = match options.backend.as_deref() {
        None | Some("gguf") => "gguf",
        Some("mlx") => "mlx",
        Some(other) => {
            return Err(Error::InvalidInput(format!(
                "unknown pull backend {other:?}; expected gguf or mlx"
            )))
        }
    };

    let (id, repo, file) = match positional {
        // Zero-config: the recommended model for the chosen backend.
        [] => match backend {
            "gguf" => (
                RECOMMENDED_MODEL_ID.to_string(),
                RECOMMENDED_GGUF_REPO.to_string(),
                Some(RECOMMENDED_GGUF_FILE.to_string()),
            ),
            _ => {
                // The mlx snapshot pull arrives in the next slice.
                return Err(Error::InvalidInput(format!(
                    "mlx pull is not wired yet; register the repo directly: \
                     local-model register <id> mlx {}",
                    crate::types::RECOMMENDED_MLX_REPO
                )));
            }
        },
        [id, repo, rest @ ..] if rest.len() <= 1 => {
            let id = valid_model_id(id)?;
            let repo = valid_repo(repo)?;
            (id, repo, rest.first().cloned())
        }
        _ => {
            return Err(Error::InvalidInput(
                "usage: local-model.pull [<id> <repo> [<file>]] [--backend gguf|mlx] [options]"
                    .into(),
            ))
        }
    };

    match backend {
        "gguf" => {
            let file = file.ok_or_else(|| {
                Error::InvalidInput("gguf pull needs a file name inside the repo".into())
            })?;
            if file.trim().is_empty()
                || file.contains('/')
                || file.contains('\\')
                || file.contains("..")
            {
                return Err(Error::InvalidInput(format!(
                    "model file must be a plain file name, got {file:?}"
                )));
            }
            if !file.ends_with(".gguf") {
                return Err(Error::InvalidInput(format!(
                    "gguf pull expects a .gguf file, got {file:?}"
                )));
            }
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
        _ => Err(Error::InvalidInput(format!(
            "mlx pull is not wired yet; register the repo directly: \
             local-model register {id} mlx {repo}"
        ))),
    }
}

fn valid_repo(repo: &str) -> Result<String> {
    let (org, name) = repo
        .split_once('/')
        .ok_or_else(|| Error::InvalidInput(format!("repo must be org/name, got {repo:?}")))?;
    if org.trim().is_empty() || name.trim().is_empty() || name.contains('/') {
        return Err(Error::InvalidInput(format!(
            "repo must be org/name, got {repo:?}"
        )));
    }
    Ok(repo.to_string())
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

/// The most recent prior exchanges fed back on `--continue`.
pub(crate) const CONTINUE_TURN_LIMIT: usize = 8;

/// `local-model.ask <app> [--model <id>] [--system <text>] [--continue]
/// [--schema <json>] [--grammar <gbnf>] <prompt…>` — validate purely;
/// inference runs at the edge. Without `--model` the ask resolves to the
/// home's default model; `--continue` feeds back this app+model's recorded
/// turns as conversation context.
pub(crate) fn decide_ask(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;

    let mut explicit_model = None;
    let mut system = None;
    let mut continued = false;
    let mut schema = None;
    let mut grammar = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                explicit_model = Some(model_id(args, i + 1)?);
                i += 2;
            }
            "--system" => {
                let value = arg(args, i + 1, "--system value")?;
                if value.trim().is_empty() {
                    return Err(Error::InvalidInput("--system must not be empty".into()));
                }
                system = Some(value);
                i += 2;
            }
            "--continue" => {
                continued = true;
                i += 1;
            }
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

    let local = state_ref::<LocalModelState>(ctx.state, "local-model")?;
    let model = resolve_model(local, explicit_model)?;
    let backend = local
        .specs
        .get(&model)
        .map(|spec| spec.backend.clone())
        .unwrap_or_default();

    if schema.is_some() && grammar.is_some() {
        return Err(Error::InvalidInput(
            "--schema and --grammar are mutually exclusive".into(),
        ));
    }
    if grammar.is_some() && backend != "llama_cpp" {
        return Err(Error::InvalidInput(format!(
            "--grammar is llama_cpp-only; the {backend} backend supports --schema"
        )));
    }
    let prompt = required_tail(args, i, "prompt")?;
    let history = if continued {
        conversation_history(local, &app, &model)
    } else {
        Vec::new()
    };

    Ok(Decision::Effect(Effect::LocalModelCall {
        app,
        model,
        prompt,
        system,
        history,
        schema,
        grammar,
    }))
}

/// The app's recorded, successful exchanges with this model — oldest first,
/// capped at the most recent [`CONTINUE_TURN_LIMIT`].
pub(crate) fn conversation_history(
    local: &LocalModelState,
    app: &str,
    model: &str,
) -> Vec<(String, String)> {
    let Some(turns) = local.turns.get(app) else {
        return Vec::new();
    };
    let mut history: Vec<(String, String)> = turns
        .iter()
        .rev()
        .filter(|turn| turn.model == model && turn.ok)
        .take(CONTINUE_TURN_LIMIT)
        .map(|turn| (turn.prompt.clone(), turn.response.clone()))
        .collect();
    history.reverse();
    history
}

/// `--model` → the home's default → a helpful error. Public within the crate:
/// the app-facing resource surface resolves models the same way.
pub(crate) fn resolve_model(local: &LocalModelState, explicit: Option<String>) -> Result<String> {
    if let Some(model) = explicit {
        if !local.specs.contains_key(&model) {
            return Err(Error::InvalidInput(format!(
                "unknown local model: {model}; register or pull it first"
            )));
        }
        return Ok(model);
    }
    if let Some(default) = &local.default_model {
        return Ok(default.clone());
    }
    if local.specs.is_empty() {
        Err(Error::InvalidInput(
            "no local models registered; run `terrane local-model pull` to fetch the \
             recommended model, or register one"
                .into(),
        ))
    } else {
        Err(Error::InvalidInput(format!(
            "no default model set; pass --model or run `local-model default <id>` \
             (registered: {})",
            local.specs.keys().cloned().collect::<Vec<_>>().join(", ")
        )))
    }
}

/// A model id is a plain token: it names a spec and a file stem at the edge.
fn model_id(args: &[String], index: usize) -> Result<String> {
    valid_model_id(&arg(args, index, "model id")?)
}

fn valid_model_id(id: &str) -> Result<String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !valid {
        return Err(Error::InvalidInput(format!(
            "model id must be [A-Za-z0-9_.-]+, got {id:?}"
        )));
    }
    Ok(id.to_string())
}

fn supported_backend(raw: String) -> Result<String> {
    let backend = raw.trim();
    if BACKENDS.contains(&backend) {
        return Ok(backend.to_string());
    }
    Err(Error::InvalidInput(format!(
        "unknown backend {backend:?}; expected one of {BACKENDS:?}"
    )))
}

fn backend_format(backend: &str) -> &'static str {
    match backend {
        "llama_cpp" => "gguf",
        "mlx" => "mlx",
        // Unknown backends are refused before this is reached.
        _ => "unknown",
    }
}

pub(crate) fn parse_spec_options(args: &[String], from: usize) -> Result<SpecOptions> {
    let mut options = SpecOptions::default();
    let mut i = from;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" => {
                options.backend = Some(arg(args, i + 1, "--backend value")?);
                i += 2;
            }
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
                    "unknown option {other:?}; expected --backend, --context, --template, \
                     --max-tokens, or --temp"
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
