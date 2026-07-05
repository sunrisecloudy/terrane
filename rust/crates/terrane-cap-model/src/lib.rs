//! The `model` capability — calls to agent CLIs (`claude`, `codex`), recorded.
//!
//! Like `net`, the call is an [`Effect`](crate::Effect) run at the edge; its
//! output is recorded as an event, so replay reproduces the conversation without
//! re-running the agent. Reacts to `app.removed`.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{Map, Value};
use terrane_cap_blob::BlobState;
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, join_tail, state_mut,
    state_ref, truncate, AppId, CapManifest, CommandCtx, CommandSpec, Decision, Effect, Error,
    EventPattern, EventRecord, EventSpec, GrantResourceSpec, ModelImagePart, ReadValue,
    RecordedCallCap, ResourceMethod, Result, StateStore,
};

mod doc;

/// The agents this capability knows how to drive.
pub const AGENTS: [&str; 2] = ["claude", "codex"];
pub const MAX_MODEL_CALLS_PER_APP: usize = 64;
pub const MAX_IMAGE_PARTS_PER_CALL: usize = 16;
pub const MAX_IMAGE_PART_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
pub const MAX_RECORDED_MODEL_CALLS_PER_RUN: usize = 4;

/// One recorded exchange with an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTurn {
    pub agent: String,
    pub prompt: String,
    pub response: String,
    pub exit_code: i32,
}

/// This capability's slice of State: a per-app transcript of turns, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelState {
    pub turns: BTreeMap<AppId, Vec<ModelTurn>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Responded {
    app: String,
    agent: String,
    prompt: String,
    response: String,
    exit_code: i32,
}

/// Build the recorded event for a completed agent call. Called by an
/// [`EffectRunner`](crate::EffectRunner) once it has run the agent, so the
/// `"model.responded"` kind and payload shape stay owned by this capability.
pub fn responded_event(
    app: &str,
    agent: &str,
    prompt: &str,
    response: String,
    exit_code: i32,
) -> Result<EventRecord> {
    encode_event(
        "model.responded",
        &Responded {
            app: app.to_string(),
            agent: agent.to_string(),
            prompt: prompt.to_string(),
            response,
            exit_code,
        },
    )
}

pub struct ModelCapability;

impl Capability for ModelCapability {
    fn namespace(&self) -> &'static str {
        "model"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec { name: "model.ask" }],
            events: vec![EventSpec {
                kind: "model.responded",
            }],
            queries: Vec::new(),
            resources: vec![ResourceMethod::Call {
                name: "ask",
                params: &["agent", "promptJsonOrText"],
            }],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "model",
                &["call"],
                "Recorded calls to supported edge agent CLIs.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::model_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "model.ask" => {
                let app = arg(args, 0, "app")?;
                let agent = arg(args, 1, "agent (claude|codex)")?;
                let prompt = join_tail(args, 2);
                // Validate purely; the agent runs at the edge.
                ensure_app_exists(ctx.bus, &app)?;
                if !AGENTS.contains(&agent.as_str()) {
                    return Err(Error::InvalidInput(format!(
                        "unknown agent {agent:?}; expected one of {AGENTS:?}"
                    )));
                }
                if prompt.trim().is_empty() {
                    return Err(Error::InvalidInput("prompt must not be empty".into()));
                }
                enforce_spend_limit(ctx.state, &app)?;
                let (prompt, image_parts) =
                    normalize_prompt_json(ctx.state, &app, &prompt, true)?;
                Ok(Decision::Effect(Effect::ModelCall {
                    app,
                    agent,
                    prompt,
                    image_parts,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "model.responded" => {
                let e: Responded = decode_event(record)?;
                state_mut::<ModelState>(state, "model")?
                    .turns
                    .entry(e.app)
                    .or_default()
                    .push(ModelTurn {
                        agent: e.agent,
                        prompt: e.prompt,
                        response: e.response,
                        exit_code: e.exit_code,
                    });
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<ModelState>(state, "model")?.turns.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "model.responded" {
            let e: Responded = decode_event(record).ok()?;
            let prompt = truncate(&e.prompt, 40);
            return Some(format!(
                "model.responded {} via {} (exit {}): {:?} → {} chars",
                e.app,
                e.agent,
                e.exit_code,
                prompt,
                e.response.len()
            ));
        }
        None
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "ask" => Ok(ReadValue::OptString(response_text_from_records(records))),
            other => Err(Error::InvalidInput(format!(
                "model.{other} is not a callable resource"
            ))),
        }
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        (method == "ask").then_some(RecordedCallCap {
            limit: MAX_RECORDED_MODEL_CALLS_PER_RUN,
            escape_hint: "call the agent from the host edge instead of an app backend loop",
        })
    }
}

#[cfg(test)]
mod tests;

struct NormalizedPrompt {
    prompt_json: String,
    image_parts: Vec<ModelImagePart>,
}

fn enforce_spend_limit(state: &dyn StateStore, app: &str) -> Result<()> {
    let count = state_ref::<ModelState>(state, "model")?
        .turns
        .get(app)
        .map(Vec::len)
        .unwrap_or(0);
    if count >= MAX_MODEL_CALLS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "model.ask per-app recorded call limit exceeded: {MAX_MODEL_CALLS_PER_APP}"
        )));
    }
    Ok(())
}

pub fn normalize_prompt_json(
    state: &dyn StateStore,
    app: &str,
    raw: &str,
    allow_images: bool,
) -> Result<(String, Vec<ModelImagePart>)> {
    let normalized = normalize_prompt(state, app, raw, allow_images)?;
    Ok((normalized.prompt_json, normalized.image_parts))
}

fn normalize_prompt(
    state: &dyn StateStore,
    app: &str,
    raw: &str,
    allow_images: bool,
) -> Result<NormalizedPrompt> {
    if raw.len() > MAX_PROMPT_BYTES {
        return Err(Error::InvalidInput(format!(
            "model prompt exceeds {MAX_PROMPT_BYTES} bytes"
        )));
    }
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return Ok(NormalizedPrompt {
            prompt_json: raw.to_string(),
            image_parts: Vec::new(),
        });
    }
    let mut value: Value = serde_json::from_str(trimmed)
        .map_err(|e| Error::InvalidInput(format!("model prompt_json must be JSON: {e}")))?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("model prompt_json must be an object".into()))?;
    reject_inline_bytes(obj)?;
    let Some(parts_value) = obj.get_mut("parts") else {
        return Ok(NormalizedPrompt {
            prompt_json: canonical_json(&value)?,
            image_parts: Vec::new(),
        });
    };
    let parts = parts_value
        .as_array_mut()
        .ok_or_else(|| Error::InvalidInput("model prompt_json.parts must be an array".into()))?;
    let mut image_parts = Vec::new();
    for part in parts {
        let part_obj = part.as_object_mut().ok_or_else(|| {
            Error::InvalidInput("each model prompt part must be an object".into())
        })?;
        reject_inline_bytes(part_obj)?;
        if let Some(text) = part_obj.get("text") {
            if !text.is_string() {
                return Err(Error::InvalidInput("model text part must be a string".into()));
            }
            continue;
        }
        if let Some(blob) = part_obj.get("blob") {
            if !allow_images {
                return Err(Error::InvalidInput(
                    "local model does not support image input for this model".into(),
                ));
            }
            if image_parts.len() >= MAX_IMAGE_PARTS_PER_CALL {
                return Err(Error::InvalidInput(format!(
                    "model image parts exceed {MAX_IMAGE_PARTS_PER_CALL} per call"
                )));
            }
            let image = image_part_from_blob(state, app, blob)?;
            validate_image_part(&image)?;
            image_parts.push(image.clone());
            let mut ref_obj = Map::new();
            if let Some(name) = &image.name {
                ref_obj.insert("name".to_string(), Value::String(name.clone()));
            }
            ref_obj.insert("hash".to_string(), Value::String(image.hash));
            ref_obj.insert("size".to_string(), Value::Number(image.size.into()));
            ref_obj.insert("mime".to_string(), Value::String(image.mime));
            part_obj.insert("blob".to_string(), Value::Object(ref_obj));
            continue;
        }
        return Err(Error::InvalidInput(
            "model prompt part must contain text or blob".into(),
        ));
    }
    Ok(NormalizedPrompt {
        prompt_json: canonical_json(&value)?,
        image_parts,
    })
}

fn image_part_from_blob(
    state: &dyn StateStore,
    app: &str,
    blob: &Value,
) -> Result<ModelImagePart> {
    if let Some(name) = blob.as_str() {
        let meta = state_ref::<BlobState>(state, "blob")?
            .blobs
            .get(app)
            .and_then(|names| names.get(name))
            .cloned()
            .ok_or_else(|| Error::KeyNotFound(app.to_string(), name.to_string()))?;
        return Ok(ModelImagePart {
            name: Some(name.to_string()),
            hash: meta.hash,
            size: meta.size,
            mime: meta.mime,
        });
    }
    let obj = blob.as_object().ok_or_else(|| {
        Error::InvalidInput("model blob part must be a blob name or metadata object".into())
    })?;
    reject_inline_bytes(obj)?;
    let hash = string_member(obj, "hash")?;
    let size = obj
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput("model blob part size must be a u64".into()))?;
    let mime = string_member(obj, "mime")?;
    let name = obj.get("name").and_then(Value::as_str).map(ToString::to_string);
    Ok(ModelImagePart {
        name,
        hash,
        size,
        mime,
    })
}

fn validate_image_part(part: &ModelImagePart) -> Result<()> {
    if part.size > MAX_IMAGE_PART_BYTES {
        return Err(Error::InvalidInput(format!(
            "model image part exceeds {MAX_IMAGE_PART_BYTES} bytes"
        )));
    }
    if !part.mime.starts_with("image/") {
        return Err(Error::InvalidInput(format!(
            "model image part mime must be image/*, got {:?}",
            part.mime
        )));
    }
    if part.hash.len() != 64
        || !part
            .hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidInput(
            "model image part hash must be lowercase SHA-256 hex".into(),
        ));
    }
    Ok(())
}

fn reject_inline_bytes(obj: &Map<String, Value>) -> Result<()> {
    for key in ["bytes", "base64", "data", "inline"] {
        if obj.contains_key(key) {
            return Err(Error::InvalidInput(format!(
                "model image parts must reference blob metadata, not inline {key}"
            )));
        }
    }
    Ok(())
}

fn string_member(obj: &Map<String, Value>, name: &str) -> Result<String> {
    obj.get(name)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidInput(format!("model blob part {name} must be a string")))
}

fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| Error::InvalidInput(format!("canonicalize model prompt_json: {e}")))
}

fn response_text_from_records(records: &[EventRecord]) -> Option<String> {
    records
        .iter()
        .rev()
        .find(|record| record.kind == "model.responded")
        .and_then(|record| decode_event::<Responded>(record).ok())
        .map(|responded| responded.response)
}
