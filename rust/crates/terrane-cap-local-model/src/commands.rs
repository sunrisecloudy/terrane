use terrane_cap_interface::{
    arg, ensure_app_exists, required_tail, state_ref, CommandCtx, Decision, Effect, Error, Result,
};

use crate::events::{chat_cleared_event, default_set_event, registered_event, removed_event};
use crate::types::{
    embed_preset, EmbeddingConfig, LocalModelSpec, LocalModelState, BACKENDS, EMBED_PRESETS,
    RECOMMENDED_EMBED_GGUF_FILE, RECOMMENDED_EMBED_GGUF_REPO, RECOMMENDED_EMBED_MODEL_ID,
    RECOMMENDED_EMBED_PRESET, RECOMMENDED_GGUF_FILE, RECOMMENDED_GGUF_REPO, RECOMMENDED_MODEL_ID,
};

/// The optional spec flags shared by `register` and `pull`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SpecOptions {
    pub backend: Option<String>,
    pub context_length: Option<u32>,
    pub chat_template: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature_milli: Option<u32>,
    pub draft_model: Option<String>,
    /// A recognized embedding-preset name; makes this spec an embedding model.
    pub embed_preset: Option<String>,
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
    ensure_draft_supported(&backend, &options)?;
    let embedding = resolve_embedding(&options)?;
    ensure_embed_backend(&backend, embedding.is_some())?;
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
        draft_model: options.draft_model,
        embedding,
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
    if options.embed_preset.is_some() && backend != "gguf" {
        return Err(Error::InvalidInput(
            "embedding models require the gguf backend (mlx embeddings are not supported yet)"
                .into(),
        ));
    }

    let (id, repo, file) = match positional {
        // Zero-config: the recommended model for the chosen backend (or the
        // recommended embedding model when `--embed` is given).
        [] => match backend {
            "gguf" if options.embed_preset.is_some() => (
                RECOMMENDED_EMBED_MODEL_ID.to_string(),
                RECOMMENDED_EMBED_GGUF_REPO.to_string(),
                Some(RECOMMENDED_EMBED_GGUF_FILE.to_string()),
            ),
            "gguf" => (
                RECOMMENDED_MODEL_ID.to_string(),
                RECOMMENDED_GGUF_REPO.to_string(),
                Some(RECOMMENDED_GGUF_FILE.to_string()),
            ),
            _ => (
                crate::types::RECOMMENDED_MLX_MODEL_ID.to_string(),
                crate::types::RECOMMENDED_MLX_REPO.to_string(),
                None,
            ),
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
            ensure_draft_supported("llama_cpp", &options)?;
            Ok(Decision::Effect(Effect::LocalModelPull {
                id,
                repo,
                backend: "gguf".to_string(),
                file: Some(file),
                context_length: options.context_length,
                chat_template: options.chat_template,
                max_tokens: options.max_tokens,
                temperature_milli: options.temperature_milli,
                draft_model: None,
                embed_preset: options.embed_preset,
            }))
        }
        _ => {
            if file.is_some() {
                return Err(Error::InvalidInput(
                    "mlx pull snapshots the whole repo; drop the file argument".into(),
                ));
            }
            Ok(Decision::Effect(Effect::LocalModelPull {
                id,
                repo,
                backend: "mlx".to_string(),
                file: None,
                context_length: options.context_length,
                chat_template: options.chat_template,
                max_tokens: options.max_tokens,
                temperature_milli: options.temperature_milli,
                draft_model: options.draft_model,
                embed_preset: None,
            }))
        }
    }
}

/// Speculative decoding is an mlx-only lever: llama_cpp asks run a single
/// in-process model, so a draft spec there would silently do nothing.
fn ensure_draft_supported(backend: &str, options: &SpecOptions) -> Result<()> {
    if options.draft_model.is_some() && backend != "mlx" {
        return Err(Error::InvalidInput(format!(
            "--draft is mlx-only (speculative decoding); the {backend} backend does not use it"
        )));
    }
    Ok(())
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

/// `ctx.resource["local-model"].askModel(model, prompt)` — positional args
/// scoped as [app, model, prompt…].
pub(crate) fn decide_ask_model(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let model = arg(args, 1, "model")?;
    let rest = args.get(2..).unwrap_or_default();
    let mut rewritten = vec![app, "--model".to_string(), model];
    rewritten.extend(rest.iter().cloned());
    decide_ask(ctx, &rewritten)
}

/// `ctx.resource["local-model"].askJson(schema, prompt)` — positional args
/// scoped as [app, schema, prompt…], answered by the default model.
pub(crate) fn decide_ask_json(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let schema = arg(args, 1, "schema")?;
    let rest = args.get(2..).unwrap_or_default();
    let mut rewritten = vec![app, "--schema".to_string(), schema];
    rewritten.extend(rest.iter().cloned());
    decide_ask(ctx, &rewritten)
}

/// `ctx.resource["local-model"].chat(prompt)` — a conversation turn: the
/// app's recorded exchanges with the default model are fed back as context.
pub(crate) fn decide_chat(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let rest = args.get(1..).unwrap_or_default();
    let mut rewritten = vec![app, "--continue".to_string()];
    rewritten.extend(rest.iter().cloned());
    decide_ask(ctx, &rewritten)
}

/// `ctx.resource["local-model"].chatModel(model, prompt)` — a conversation
/// turn with an explicitly named registered model.
pub(crate) fn decide_chat_model(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let model = arg(args, 1, "model")?;
    let rest = args.get(2..).unwrap_or_default();
    let mut rewritten = vec![app, "--model".to_string(), model, "--continue".to_string()];
    rewritten.extend(rest.iter().cloned());
    decide_ask(ctx, &rewritten)
}

/// `local-model.embed <app> [--model <id>] [--query] <text…>` — encode text
/// into a dense vector with a registered embedding model; the vector is
/// computed at the edge and recorded. Without `--model` the embed resolves to
/// the home's default embedding model. `--query` applies the model's query
/// prefix (search side) instead of the document prefix (index side).
pub(crate) fn decide_embed(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;

    let mut explicit_model = None;
    let mut query = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                explicit_model = Some(model_id(args, i + 1)?);
                i += 2;
            }
            "--query" => {
                query = true;
                i += 1;
            }
            _ => break,
        }
    }

    let local = state_ref::<LocalModelState>(ctx.state, "local-model")?;
    let model = resolve_embed_model(local, explicit_model)?;
    let text = required_tail(args, i, "text")?;

    Ok(Decision::Effect(Effect::LocalModelEmbed {
        app,
        model,
        texts: vec![text],
        query,
    }))
}

/// `ctx.resource["local-model"].embedQuery(text)` — embed search-side text
/// (applies the model's query prefix), answered by the default embedding model.
pub(crate) fn decide_embed_query(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let rest = args.get(1..).unwrap_or_default();
    let mut rewritten = vec![app, "--query".to_string()];
    rewritten.extend(rest.iter().cloned());
    decide_embed(ctx, &rewritten)
}

/// `ctx.resource["local-model"].embedModel(model, text)` — embed document-side
/// text with an explicitly named embedding model.
pub(crate) fn decide_embed_model(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let model = arg(args, 1, "model")?;
    let rest = args.get(2..).unwrap_or_default();
    let mut rewritten = vec![app, "--model".to_string(), model];
    rewritten.extend(rest.iter().cloned());
    decide_embed(ctx, &rewritten)
}

/// `--model` → the home's default embedding model → a helpful error. Also
/// refuses a generation model asked to embed.
pub(crate) fn resolve_embed_model(
    local: &LocalModelState,
    explicit: Option<String>,
) -> Result<String> {
    if let Some(model) = explicit {
        return match local.specs.get(&model) {
            None => Err(Error::InvalidInput(format!(
                "unknown local model: {model}; register or pull it first"
            ))),
            Some(spec) if spec.embedding.is_none() => Err(Error::InvalidInput(format!(
                "{model} is not an embedding model; pull one with `local-model pull --embed`"
            ))),
            Some(_) => Ok(model),
        };
    }
    if let Some(default) = &local.default_embed_model {
        return Ok(default.clone());
    }
    Err(Error::InvalidInput(
        "no embedding model registered; run `terrane local-model pull --embed` to fetch the \
         recommended one"
            .into(),
    ))
}

/// Resolve the embedding config named by `--embed`/`--embed-preset`, if any.
fn resolve_embedding(options: &SpecOptions) -> Result<Option<EmbeddingConfig>> {
    match &options.embed_preset {
        None => Ok(None),
        Some(name) => embed_preset(name).map(Some).ok_or_else(|| {
            Error::InvalidInput(format!(
                "unknown embed preset {name:?}; expected one of {EMBED_PRESETS:?}"
            ))
        }),
    }
}

/// Embeddings only run on the llama_cpp backend today; refuse an mlx embed spec.
fn ensure_embed_backend(backend: &str, is_embedding: bool) -> Result<()> {
    if is_embedding && backend != "llama_cpp" {
        return Err(Error::InvalidInput(format!(
            "embedding models require the llama_cpp backend; got {backend}"
        )));
    }
    Ok(())
}

/// `ctx.resource["local-model"].pullModel(repo[, file])` — download weights
/// from Hugging Face and register them, exactly like the admin `pull` command
/// but app-initiated (still behind the app's local-model grant). The model id
/// derives from the repo name; a `.gguf` file selects the llama_cpp backend,
/// no file snapshots the repo for mlx.
pub(crate) fn decide_pull_model(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let _app = arg(args, 0, "app")?;
    let repo = arg(args, 1, "repo")?;
    let file = args
        .get(2)
        .map(String::as_str)
        .filter(|f| !f.trim().is_empty());
    let id = model_id_from_repo(&repo)?;
    let mut rewritten = vec![id, repo.clone()];
    match file {
        Some(file) => rewritten.push(file.to_string()),
        None => {
            rewritten.push("--backend".to_string());
            rewritten.push("mlx".to_string());
        }
    }
    decide_pull(ctx, &rewritten)
}

/// `ctx.resource["local-model"].resetChat()` — start a fresh conversation:
/// records the transcript-clearing event for this app.
pub(crate) fn decide_reset_chat(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    Ok(Decision::Commit(vec![chat_cleared_event(&app)?]))
}

/// `ctx.resource["local-model"].models()` — the registered models as a JSON
/// array (id, backend, default flag), for model-picker UIs.
pub(crate) fn read_models(
    state: &dyn terrane_cap_interface::StateStore,
    _args: &[String],
) -> Result<terrane_cap_interface::ReadValue> {
    let local = state_ref::<LocalModelState>(state, "local-model")?;
    let models: Vec<serde_json::Value> = local
        .specs
        .iter()
        .map(|(id, spec)| {
            serde_json::json!({
                "id": id,
                "backend": spec.backend,
                "default": local.default_model.as_deref() == Some(id.as_str()),
            })
        })
        .collect();
    let encoded = serde_json::to_string(&models)
        .map_err(|e| Error::InvalidInput(format!("model list encode failed: {e}")))?;
    Ok(terrane_cap_interface::ReadValue::OptString(Some(encoded)))
}

/// Derive a registerable model id from a Hugging Face repo name:
/// `unsloth/Qwen3.5-0.8B-GGUF` → `qwen3.5-0.8b-gguf`.
fn model_id_from_repo(repo: &str) -> Result<String> {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let id: String = name
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    valid_model_id(&id)
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
            "--draft" => {
                let value = arg(args, i + 1, "--draft value")?;
                if value.trim().is_empty() {
                    return Err(Error::InvalidInput("--draft must not be empty".into()));
                }
                options.draft_model = Some(value);
                i += 2;
            }
            // Bare `--embed` uses the recommended preset; `--embed-preset <name>`
            // selects a specific encoder family.
            "--embed" => {
                options.embed_preset = Some(RECOMMENDED_EMBED_PRESET.to_string());
                i += 1;
            }
            "--embed-preset" => {
                let value = arg(args, i + 1, "--embed-preset value")?;
                if embed_preset(&value).is_none() {
                    return Err(Error::InvalidInput(format!(
                        "unknown embed preset {value:?}; expected one of {EMBED_PRESETS:?}"
                    )));
                }
                options.embed_preset = Some(value);
                i += 2;
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown option {other:?}; expected --backend, --context, --template, \
                     --max-tokens, --temp, --draft, --embed, or --embed-preset"
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
