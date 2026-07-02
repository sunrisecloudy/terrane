use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::types::{LocalModelSpec, LocalModelState, LocalModelTurn};

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
        },
    )
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

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "local-model.registered" => {
            let e: Registered = decode_event(record)?;
            let local = state_mut::<LocalModelState>(state, "local-model")?;
            let id = e.id.clone();
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
                },
            );
            // The first registered model becomes the default automatically.
            if local.default_model.is_none() {
                local.default_model = Some(id);
            }
        }
        "local-model.removed" => {
            let e: Removed = decode_event(record)?;
            let local = state_mut::<LocalModelState>(state, "local-model")?;
            local.specs.remove(&e.id);
            if local.default_model.as_deref() == Some(e.id.as_str()) {
                local.default_model = None;
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
                "local-model.registered {} ({}/{}) at {}",
                e.id, e.backend, e.format, e.local_path
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
