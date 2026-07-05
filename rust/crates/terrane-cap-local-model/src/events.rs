use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::types::{EmbeddingConfig, LocalModelSpec, LocalModelState, LocalModelTurn};

/// Borsh mirror of [`EmbeddingConfig`] so the public type stays free of the
/// wire derive (the same split `LocalModelSpec`/`Registered` uses).
#[derive(BorshSerialize, BorshDeserialize)]
struct EmbeddingConfigWire {
    pooling: String,
    query_prefix: String,
    document_prefix: String,
    normalize: bool,
    dim: Option<u32>,
}

impl EmbeddingConfigWire {
    fn from_config(config: &EmbeddingConfig) -> Self {
        EmbeddingConfigWire {
            pooling: config.pooling.clone(),
            query_prefix: config.query_prefix.clone(),
            document_prefix: config.document_prefix.clone(),
            normalize: config.normalize,
            dim: config.dim,
        }
    }

    fn into_config(self) -> EmbeddingConfig {
        EmbeddingConfig {
            pooling: self.pooling,
            query_prefix: self.query_prefix,
            document_prefix: self.document_prefix,
            normalize: self.normalize,
            dim: self.dim,
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Registered {
    id: String,
    backend: String,
    format: String,
    local_path: String,
    context_length: Option<u32>,
    chat_template: Option<String>,
    max_tokens: Option<u32>,
    temperature_milli: Option<u32>,
    source: Option<String>,
    size_bytes: Option<u64>,
    draft_model: Option<String>,
    embedding: Option<EmbeddingConfigWire>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Embedded {
    app: String,
    model: String,
    query: bool,
    dim: u32,
    vectors: Vec<Vec<f32>>,
    duration_ms: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    id: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct DefaultSet {
    id: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct ChatCleared {
    app: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Responded {
    app: String,
    model: String,
    prompt: String,
    system: Option<String>,
    continued: bool,
    response: String,
    ok: bool,
    constraint: Option<String>,
    token_count: u32,
    duration_ms: u64,
}

/// Everything one completed generation records; the effect runner fills it so
/// the `"local-model.responded"` payload shape stays owned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RespondedRecord {
    pub app: String,
    pub model: String,
    pub prompt: String,
    pub system: Option<String>,
    pub continued: bool,
    pub response: String,
    pub ok: bool,
    /// `"schema-mask"`, `"schema-guided"`, or `"grammar"` when constrained.
    pub constraint: Option<String>,
    pub token_count: u32,
    pub duration_ms: u64,
}

/// Everything one completed embedding run records; the effect runner fills it
/// so the `"local-model.embedded"` payload shape stays owned by this crate.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedRecord {
    pub app: String,
    pub model: String,
    /// Whether the query prefix (vs the document prefix) was applied.
    pub query: bool,
    /// The dimension of each returned vector (after any truncation).
    pub dim: u32,
    /// One vector per input text, in order.
    pub vectors: Vec<Vec<f32>>,
    pub duration_ms: u64,
}

/// Build the recorded event for a registered (or re-registered) model spec.
/// Also called by an `EffectRunner` once a pull has downloaded the weights, so
/// the `"local-model.registered"` kind and payload shape stay owned here.
pub fn registered_event(id: &str, spec: &LocalModelSpec) -> Result<EventRecord> {
    encode_event(
        "local-model.registered",
        &Registered {
            id: id.to_string(),
            backend: spec.backend.clone(),
            format: spec.format.clone(),
            local_path: spec.local_path.clone(),
            context_length: spec.context_length,
            chat_template: spec.chat_template.clone(),
            max_tokens: spec.max_tokens,
            temperature_milli: spec.temperature_milli,
            source: spec.source.clone(),
            size_bytes: spec.size_bytes,
            draft_model: spec.draft_model.clone(),
            embedding: spec
                .embedding
                .as_ref()
                .map(EmbeddingConfigWire::from_config),
        },
    )
}

/// Build the recorded event for one completed embedding run. The pooled vectors
/// are the recorded result so replay never re-runs inference — but `fold` does
/// not keep them in `State` (floats aren't `Eq`); the caller consumes them at
/// commit time via [`vectors_from_records`].
pub fn embedded_event(record: &EmbeddedRecord) -> Result<EventRecord> {
    encode_event(
        "local-model.embedded",
        &Embedded {
            app: record.app.clone(),
            model: record.model.clone(),
            query: record.query,
            dim: record.dim,
            vectors: record.vectors.clone(),
            duration_ms: record.duration_ms,
        },
    )
}

/// The vectors from a freshly committed embedding batch, for the `embed` call
/// surface to hand back to the caller.
pub(crate) fn vectors_from_records(records: &[EventRecord]) -> Option<Vec<Vec<f32>>> {
    records
        .iter()
        .rev()
        .find(|record| record.kind == "local-model.embedded")
        .and_then(|record| decode_event::<Embedded>(record).ok())
        .map(|embedded| embedded.vectors)
}

/// Build the recorded event for an unregistered model spec.
pub fn removed_event(id: &str) -> Result<EventRecord> {
    encode_event("local-model.removed", &Removed { id: id.to_string() })
}

/// Build the recorded event for an explicit default-model change.
pub fn default_set_event(id: &str) -> Result<EventRecord> {
    encode_event(
        "local-model.default-set",
        &DefaultSet { id: id.to_string() },
    )
}

/// Build the recorded event that clears an app's conversation transcript
/// (a "new chat" — later `--continue`/`chat` calls start fresh).
pub fn chat_cleared_event(app: &str) -> Result<EventRecord> {
    encode_event(
        "local-model.chat-cleared",
        &ChatCleared {
            app: app.to_string(),
        },
    )
}

/// Build the recorded event for one completed local inference.
pub fn responded_event(record: &RespondedRecord) -> Result<EventRecord> {
    encode_event(
        "local-model.responded",
        &Responded {
            app: record.app.clone(),
            model: record.model.clone(),
            prompt: record.prompt.clone(),
            system: record.system.clone(),
            continued: record.continued,
            response: record.response.clone(),
            ok: record.ok,
            constraint: record.constraint.clone(),
            token_count: record.token_count,
            duration_ms: record.duration_ms,
        },
    )
}

/// The freshly registered model id inside a committed batch (used by the
/// `pullModel` call surface to hand the new id back to the app).
pub(crate) fn registered_id_from_records(records: &[EventRecord]) -> Option<String> {
    records
        .iter()
        .rev()
        .find(|record| record.kind == "local-model.registered")
        .and_then(|record| decode_event::<Registered>(record).ok())
        .map(|registered| registered.id)
}

/// The recorded response text inside a freshly committed batch (used by the
/// `ctx.resource["local-model"]` call surface to hand the answer back).
pub(crate) fn response_text_from_records(records: &[EventRecord]) -> Option<String> {
    records
        .iter()
        .rev()
        .find(|record| record.kind == "local-model.responded")
        .and_then(|record| decode_event::<Responded>(record).ok())
        .map(|responded| responded.response)
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "local-model.registered" => {
            let e: Registered = decode_event(record)?;
            let local = state_mut::<LocalModelState>(state, "local-model")?;
            let id = e.id.clone();
            let embedding = e.embedding.map(EmbeddingConfigWire::into_config);
            let is_embedding = embedding.is_some();
            local.specs.insert(
                e.id,
                LocalModelSpec {
                    backend: e.backend,
                    format: e.format,
                    local_path: e.local_path,
                    context_length: e.context_length,
                    chat_template: e.chat_template,
                    max_tokens: e.max_tokens,
                    temperature_milli: e.temperature_milli,
                    source: e.source,
                    size_bytes: e.size_bytes,
                    draft_model: e.draft_model,
                    embedding,
                },
            );
            // The first registered model of each kind becomes that kind's
            // default: embedding models set the embed default, generation models
            // the chat default — never crossing over.
            if is_embedding {
                if local.default_embed_model.is_none() {
                    local.default_embed_model = Some(id);
                }
            } else if local.default_model.is_none() {
                local.default_model = Some(id);
            }
        }
        "local-model.embedded" => {
            // A derived read-model: the recorded vectors are consumed by the
            // caller at commit time and deliberately never enter State (floats
            // aren't `Eq`, so keeping them would break replay identity).
        }
        "local-model.removed" => {
            let e: Removed = decode_event(record)?;
            let local = state_mut::<LocalModelState>(state, "local-model")?;
            local.specs.remove(&e.id);
            if local.default_model.as_deref() == Some(e.id.as_str()) {
                local.default_model = None;
            }
            if local.default_embed_model.as_deref() == Some(e.id.as_str()) {
                local.default_embed_model = None;
            }
        }
        "local-model.default-set" => {
            let e: DefaultSet = decode_event(record)?;
            state_mut::<LocalModelState>(state, "local-model")?.default_model = Some(e.id);
        }
        "local-model.responded" => {
            let e: Responded = decode_event(record)?;
            state_mut::<LocalModelState>(state, "local-model")?
                .turns
                .entry(e.app)
                .or_default()
                .push(LocalModelTurn {
                    model: e.model,
                    prompt: e.prompt,
                    system: e.system,
                    continued: e.continued,
                    response: e.response,
                    ok: e.ok,
                    constraint: e.constraint,
                    token_count: e.token_count,
                    duration_ms: e.duration_ms,
                });
        }
        "local-model.chat-cleared" => {
            let e: ChatCleared = decode_event(record)?;
            state_mut::<LocalModelState>(state, "local-model")?
                .turns
                .remove(&e.app);
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            // Transcripts are app-scoped and go with the app; specs are global
            // machine configuration and stay.
            state_mut::<LocalModelState>(state, "local-model")?
                .turns
                .remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "local-model.registered" => {
            let e: Registered = decode_event(record).ok()?;
            Some(format!(
                "local-model.registered {} ({}/{}{}) at {}",
                e.id,
                e.backend,
                e.format,
                if e.embedding.is_some() {
                    ", embedding"
                } else {
                    ""
                },
                e.local_path
            ))
        }
        "local-model.embedded" => {
            let e: Embedded = decode_event(record).ok()?;
            Some(format!(
                "local-model.embedded {} via {} ({} vector(s), {}-d, {}, {}ms)",
                e.app,
                e.model,
                e.vectors.len(),
                e.dim,
                if e.query { "query" } else { "document" },
                e.duration_ms
            ))
        }
        "local-model.removed" => {
            let e: Removed = decode_event(record).ok()?;
            Some(format!("local-model.removed {}", e.id))
        }
        "local-model.default-set" => {
            let e: DefaultSet = decode_event(record).ok()?;
            Some(format!("local-model.default-set {}", e.id))
        }
        "local-model.chat-cleared" => {
            let e: ChatCleared = decode_event(record).ok()?;
            Some(format!("local-model.chat-cleared {}", e.app))
        }
        "local-model.responded" => {
            let e: Responded = decode_event(record).ok()?;
            let prompt = terrane_cap_interface::truncate(&e.prompt, 40);
            let constrained = match &e.constraint {
                Some(mode) => format!(", constrained({mode})"),
                None => String::new(),
            };
            Some(format!(
                "local-model.responded {} via {} ({}{}{}{}, {} tokens, {}ms): {:?} → {} chars",
                e.app,
                e.model,
                if e.ok { "ok" } else { "failed" },
                constrained,
                if e.continued { ", continued" } else { "" },
                if e.system.is_some() { ", system" } else { "" },
                e.token_count,
                e.duration_ms,
                prompt,
                e.response.len()
            ))
        }
        _ => None,
    }
}
