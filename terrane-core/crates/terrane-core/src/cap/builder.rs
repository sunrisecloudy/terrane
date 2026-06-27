//! The `builder` capability — app generation drafts driven by agent CLIs.
//!
//! A build request is effectful: the core validates the request, the edge asks
//! Codex for a Terrane bundle, and replay rebuilds the recorded draft from
//! `builder.*` events without re-running Codex.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use borsh::{BorshDeserialize, BorshSerialize};
use nanoserde::{DeJson, SerJson};
use terrane_domain::{Error, EventRecord, Result};

use super::{arg, Capability};
use crate::{decode_event, encode_event, Decision, Effect, State};

pub const DEFAULT_AGENT: &str = "codex";
pub const SUPPORTED_AGENTS: [&str; 1] = [DEFAULT_AGENT];
const SUPPORTED_EXTENSIONS: &[&str] = &["html", "htm", "css", "js", "mjs", "json", "svg"];
const MAX_FILES: usize = 48;
const MAX_TOTAL_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuilderState {
    pub drafts: BTreeMap<String, BuilderDraft>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuilderDraft {
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub files: Vec<BuilderFile>,
    pub error: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, DeJson, SerJson,
)]
pub struct BuilderFile {
    pub path: String,
    pub content: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Requested {
    id: String,
    app_id: String,
    name: String,
    prompt: String,
    agent: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Generated {
    id: String,
    files: Vec<BuilderFile>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Failed {
    id: String,
    error: String,
}

#[derive(DeJson)]
struct GeneratedPayload {
    files: Vec<BuilderFile>,
}

#[derive(DeJson)]
struct Manifest {
    #[nserde(default)]
    id: String,
    #[nserde(default)]
    name: String,
    #[nserde(default)]
    backend: String,
    #[nserde(default)]
    ui: String,
    #[nserde(default)]
    resources: Vec<String>,
}

#[derive(SerJson)]
struct DraftJson {
    id: String,
    #[nserde(rename = "appId")]
    app_id: String,
    name: String,
    prompt: String,
    agent: String,
    status: String,
    error: String,
    files: Vec<BuilderFile>,
}

pub struct BuilderCapability;

impl Capability for BuilderCapability {
    fn namespace(&self) -> &'static str {
        "builder"
    }

    fn decide(&self, _state: &State, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "builder.generate" => {
                let id = validate_id(&arg(args, 0, "draft id")?, "draft id")?;
                let app_id = validate_id(&arg(args, 1, "app id")?, "app id")?;
                let name = non_empty(arg(args, 2, "app name")?, "app name")?;
                let agent = match args.get(3).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    Some(agent) if SUPPORTED_AGENTS.contains(&agent) => agent.to_string(),
                    Some(agent) => {
                        return Err(Error::InvalidInput(format!(
                            "unsupported builder agent {agent:?}; expected {SUPPORTED_AGENTS:?}"
                        )))
                    }
                    None => DEFAULT_AGENT.to_string(),
                };
                let prompt = non_empty(args.get(4..).unwrap_or_default().join(" "), "prompt")?;
                Ok(Decision::Effect(Effect::BuildAppWithAgent {
                    draft_id: id,
                    app_id,
                    name,
                    agent,
                    prompt,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "builder.requested" => {
                let e: Requested = decode_event(record)?;
                state.builder.drafts.insert(
                    e.id.clone(),
                    BuilderDraft {
                        id: e.id,
                        app_id: e.app_id,
                        name: e.name,
                        prompt: e.prompt,
                        agent: e.agent,
                        files: Vec::new(),
                        error: None,
                    },
                );
            }
            "builder.generated" => {
                let e: Generated = decode_event(record)?;
                let draft = state.builder.drafts.entry(e.id.clone()).or_default();
                draft.id = e.id;
                draft.files = e.files;
                draft.error = None;
            }
            "builder.failed" => {
                let e: Failed = decode_event(record)?;
                let draft = state.builder.drafts.entry(e.id.clone()).or_default();
                draft.id = e.id;
                draft.files.clear();
                draft.error = Some(e.error);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "builder.requested" => {
                let e: Requested = decode_event(record).ok()?;
                Some(format!(
                    "builder.requested {} via {}: {:?}",
                    e.app_id,
                    e.agent,
                    truncate(&e.prompt, 48)
                ))
            }
            "builder.generated" => {
                let e: Generated = decode_event(record).ok()?;
                Some(format!(
                    "builder.generated {} ({} files)",
                    e.id,
                    e.files.len()
                ))
            }
            "builder.failed" => {
                let e: Failed = decode_event(record).ok()?;
                Some(format!(
                    "builder.failed {}: {}",
                    e.id,
                    truncate(&e.error, 80)
                ))
            }
            _ => None,
        }
    }
}

pub fn requested_event(
    id: &str,
    app_id: &str,
    name: &str,
    prompt: &str,
    agent: &str,
) -> Result<EventRecord> {
    encode_event(
        "builder.requested",
        &Requested {
            id: id.to_string(),
            app_id: app_id.to_string(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            agent: agent.to_string(),
        },
    )
}

pub fn generated_event(id: &str, files: Vec<BuilderFile>) -> Result<EventRecord> {
    encode_event(
        "builder.generated",
        &Generated {
            id: id.to_string(),
            files,
        },
    )
}

pub fn failed_event(id: &str, error: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "builder.failed",
        &Failed {
            id: id.to_string(),
            error: error.into(),
        },
    )
}

pub fn draft_json(draft: &BuilderDraft) -> String {
    DraftJson {
        id: draft.id.clone(),
        app_id: draft.app_id.clone(),
        name: draft.name.clone(),
        prompt: draft.prompt.clone(),
        agent: draft.agent.clone(),
        status: if draft.error.is_some() {
            "failed".to_string()
        } else if draft.files.is_empty() {
            "requested".to_string()
        } else {
            "generated".to_string()
        },
        error: draft.error.clone().unwrap_or_default(),
        files: draft.files.clone(),
    }
    .serialize_json()
}

pub fn codex_prompt(app_id: &str, name: &str, user_prompt: &str) -> String {
    format!(
        "Generate a complete Terrane app bundle for this user request.\n\n\
         User request:\n{user_prompt}\n\n\
         Hard requirements:\n\
         - Return only a JSON object, no markdown, no prose.\n\
         - Shape: {{\"files\":[{{\"path\":\"manifest.json\",\"content\":\"...\"}}]}}\n\
         - manifest.json must use id {app_id:?}, name {name:?}, version \"0.1.0\", \
           backend \"main.js\", ui \"index.html\".\n\
         - Include manifest.json, main.js, index.html, and style.css.\n\
         - Backend runs in Terrane QuickJS. Define a global `actions` object or \
           `handle(input)`. It must return strings only.\n\
         - Actions receive string args and may use `ctx.resource.kv.get(key)`, \
           `ctx.resource.kv.set(key, value)`, `ctx.resource.kv.rm(key)`, and \
           `ctx.resource.kv.all()` when manifest.resources includes \"kv\".\n\
         - UI runs in a webview. It may call `window.terrane.invoke(verb, ...args)` \
           and must work if that bridge is missing.\n\
         - Use only relative local files. No CDN, no fetch, no external packages, \
           no build step, no dynamic import, no eval.\n\
         - Resource names allowed in manifest.resources: \"kv\" and \"crdt\" only. \
           Leave resources empty unless persistence is necessary.\n\
         - Supported file extensions: html, htm, css, js, mjs, json, svg.\n"
    )
}

pub fn parse_generated_files(raw: &str, app_id: &str, name: &str) -> Result<Vec<BuilderFile>> {
    let json = extract_json_object(raw)?;
    let payload = GeneratedPayload::deserialize_json(json)
        .map_err(|e| Error::InvalidInput(format!("builder output JSON: {e}")))?;
    validate_files(payload.files, app_id, name)
}

pub fn validate_files(
    files: Vec<BuilderFile>,
    app_id: &str,
    name: &str,
) -> Result<Vec<BuilderFile>> {
    if files.is_empty() {
        return Err(Error::InvalidInput("builder output has no files".into()));
    }
    if files.len() > MAX_FILES {
        return Err(Error::InvalidInput(format!(
            "builder output has too many files: {} > {MAX_FILES}",
            files.len()
        )));
    }

    let mut total = 0usize;
    let mut out = BTreeMap::new();
    for file in files {
        let path = normalize_rel_path(&file.path)?;
        let ext = extension(&path).ok_or_else(|| {
            Error::InvalidInput(format!("builder output file has no extension: {path}"))
        })?;
        if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(Error::InvalidInput(format!(
                "unsupported builder output file extension: {path}"
            )));
        }
        total = total.saturating_add(file.content.len());
        if total > MAX_TOTAL_BYTES {
            return Err(Error::InvalidInput(format!(
                "builder output is too large: {total} bytes > {MAX_TOTAL_BYTES}"
            )));
        }
        if out
            .insert(
                path.clone(),
                BuilderFile {
                    path,
                    content: file.content,
                },
            )
            .is_some()
        {
            return Err(Error::InvalidInput("duplicate builder output file".into()));
        }
    }

    let manifest_text = out
        .get("manifest.json")
        .ok_or_else(|| Error::InvalidInput("builder output missing manifest.json".into()))?;
    let manifest = Manifest::deserialize_json(&manifest_text.content)
        .map_err(|e| Error::InvalidInput(format!("builder manifest.json: {e}")))?;
    if manifest.id.trim() != app_id {
        return Err(Error::InvalidInput(format!(
            "builder manifest id {:?} must match requested app id {app_id:?}",
            manifest.id
        )));
    }
    if manifest.name.trim() != name {
        return Err(Error::InvalidInput(format!(
            "builder manifest name {:?} must match requested app name {name:?}",
            manifest.name
        )));
    }
    let backend = normalize_rel_path(&manifest.backend)
        .map_err(|e| Error::InvalidInput(format!("manifest.backend is invalid: {e}")))?;
    let ui = normalize_rel_path(&manifest.ui)
        .map_err(|e| Error::InvalidInput(format!("manifest.ui is invalid: {e}")))?;
    if !matches!(extension(&backend).as_deref(), Some("js" | "mjs")) {
        return Err(Error::InvalidInput(format!(
            "manifest.backend must reference a JS file: {backend}"
        )));
    }
    if !matches!(extension(&ui).as_deref(), Some("html" | "htm")) {
        return Err(Error::InvalidInput(format!(
            "manifest.ui must reference an HTML file: {ui}"
        )));
    }
    if !out.contains_key(&backend) {
        return Err(Error::InvalidInput(format!(
            "manifest.backend references missing file: {backend}"
        )));
    }
    if !out.contains_key(&ui) {
        return Err(Error::InvalidInput(format!(
            "manifest.ui references missing file: {ui}"
        )));
    }
    for resource in manifest.resources {
        if !matches!(resource.as_str(), "kv" | "crdt") {
            return Err(Error::InvalidInput(format!(
                "unsupported generated app resource: {resource}"
            )));
        }
    }

    Ok(out.into_values().collect())
}

fn validate_id(raw: &str, label: &str) -> Result<String> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(Error::InvalidInput(format!("{label} must not be empty")));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "{label} is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(id.to_string())
}

fn non_empty(raw: String, label: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        Err(Error::InvalidInput(format!("{label} must not be empty")))
    } else {
        Ok(value.to_string())
    }
}

fn extract_json_object(raw: &str) -> Result<&str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed);
    }
    let start = raw
        .find('{')
        .ok_or_else(|| Error::InvalidInput("builder output did not contain JSON".into()))?;
    let end = raw.rfind('}').ok_or_else(|| {
        Error::InvalidInput("builder output did not contain complete JSON".into())
    })?;
    if end <= start {
        return Err(Error::InvalidInput(
            "builder output JSON range is invalid".into(),
        ));
    }
    Ok(&raw[start..=end])
}

fn normalize_rel_path(input: &str) -> Result<String> {
    if input.trim().is_empty() {
        return Err(Error::InvalidInput("path must not be empty".into()));
    }
    if input.contains('\\') {
        return Err(Error::InvalidInput(format!(
            "path must use '/' separators: {input}"
        )));
    }
    let mut parts = Vec::new();
    for component in Path::new(input).components() {
        match component {
            Component::Normal(part) => {
                let s = part.to_str().ok_or_else(|| {
                    Error::InvalidInput(format!("path is not valid UTF-8: {input}"))
                })?;
                if !s.is_empty() {
                    parts.push(s.to_string());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::InvalidInput(format!(
                    "parent-dir components are not allowed: {input}"
                )))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::InvalidInput(format!(
                    "absolute paths are not allowed: {input}"
                )))
            }
        }
    }
    if parts.is_empty() {
        return Err(Error::InvalidInput(format!(
            "path must name a file: {input}"
        )));
    }
    Ok(parts.join("/"))
}

fn extension(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let (_, ext) = file.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated_json() -> String {
        r#"{"files":[
{"path":"manifest.json","content":"{\"id\":\"demo\",\"name\":\"Demo\",\"version\":\"0.1.0\",\"backend\":\"main.js\",\"ui\":\"index.html\",\"resources\":[\"kv\"]}"},
{"path":"main.js","content":"var actions={hello:{summary:\"Say hello.\",args:[],run:function(){return \"hi\";}}};"},
{"path":"index.html","content":"<!doctype html><title>Demo</title><script src=\"app.js\"></script>"},
{"path":"style.css","content":"body { font-family: system-ui; }"}
]}"#
        .to_string()
    }

    #[test]
    fn parses_and_validates_generated_bundle_files() {
        let files = parse_generated_files(&generated_json(), "demo", "Demo").unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].path, "index.html");
        assert!(files.iter().any(|f| f.path == "manifest.json"));
    }

    #[test]
    fn rejects_unsafe_or_mismatched_generated_files() {
        let bad_path = generated_json().replace("style.css", "../escape.css");
        assert!(parse_generated_files(&bad_path, "demo", "Demo")
            .unwrap_err()
            .to_string()
            .contains("parent-dir"));

        let bad_id =
            generated_json().replace("\\\"id\\\":\\\"demo\\\"", "\\\"id\\\":\\\"other\\\"");
        assert!(parse_generated_files(&bad_id, "demo", "Demo")
            .unwrap_err()
            .to_string()
            .contains("must match"));

        let bad_resource = generated_json().replace("\\\"kv\\\"", "\\\"net\\\"");
        assert!(parse_generated_files(&bad_resource, "demo", "Demo")
            .unwrap_err()
            .to_string()
            .contains("unsupported"));
    }
}
