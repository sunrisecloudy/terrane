use std::collections::BTreeMap;
use std::path::{Component, Path};

use nanoserde::{DeJson, SerJson};
use terrane_core::cap::app::AppRecord;
use terrane_core::cap::host::{run_memory_backend, BundleManifest, MemoryBackendBundle};
use terrane_core::{fold_records_in_memory, State};

const SUPPORTED_EXTENSIONS: &[&str] = &["html", "htm", "css", "js", "mjs", "json", "svg"];

#[derive(Clone, Debug, PartialEq, Eq, DeJson, SerJson)]
pub struct PreviewFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct PreviewCreated {
    pub id: String,
    #[nserde(rename = "frameUrl")]
    pub frame_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct PreviewAsset {
    pub content: String,
    #[nserde(rename = "contentType")]
    pub content_type: String,
}

#[derive(Default)]
pub struct PreviewStore {
    counter: u64,
    previews: BTreeMap<String, Preview>,
}

struct Preview {
    id: String,
    files: BTreeMap<String, String>,
    ui: String,
    state: State,
    bundle: MemoryBackendBundle,
}

#[derive(DeJson)]
struct PreviewFilesPayload {
    files: Vec<PreviewFile>,
}

#[derive(DeJson)]
struct PreviewManifest {
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

impl PreviewStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_preview_from_json(
        &mut self,
        files_json: &str,
        base_state: &State,
    ) -> Result<PreviewCreated, String> {
        self.create_preview(parse_files_payload(files_json)?, base_state)
    }

    pub fn create_preview_json_from_json(
        &mut self,
        files_json: &str,
        base_state: &State,
    ) -> Result<String, String> {
        Ok(self
            .create_preview_from_json(files_json, base_state)?
            .serialize_json())
    }

    pub fn create_preview(
        &mut self,
        files: Vec<PreviewFile>,
        base_state: &State,
    ) -> Result<PreviewCreated, String> {
        let files = validate_files(files)?;
        let manifest_text = files
            .get("manifest.json")
            .ok_or_else(|| "missing manifest.json".to_string())?;
        let manifest = parse_manifest(manifest_text)?;
        validate_manifest_id(&manifest.id)?;

        let ui =
            normalize_rel_path(&manifest.ui).map_err(|e| format!("manifest.ui is invalid: {e}"))?;
        let backend = normalize_rel_path(&manifest.backend)
            .map_err(|e| format!("manifest.backend is invalid: {e}"))?;
        if !files.contains_key(&ui) {
            return Err(format!("manifest.ui references missing file: {ui}"));
        }
        let backend_source = files
            .get(&backend)
            .cloned()
            .ok_or_else(|| format!("manifest.backend references missing file: {backend}"))?;

        self.counter += 1;
        let id = format!("preview-{}-{}", manifest.id.trim(), self.counter);
        let name = match manifest.name.trim() {
            "" => manifest.id.trim().to_string(),
            name => name.to_string(),
        };
        let mut state = base_state.clone();
        state.app.apps.insert(
            id.clone(),
            AppRecord {
                id: id.clone(),
                name,
                source: None,
            },
        );
        let bundle = MemoryBackendBundle {
            source: backend_source,
            name: manifest.name.clone(),
            resources: manifest.resources.clone(),
        };
        self.previews.insert(
            id.clone(),
            Preview {
                id: id.clone(),
                files,
                ui,
                state,
                bundle,
            },
        );

        Ok(PreviewCreated {
            frame_url: format!("terrane-preview://{id}/frame/"),
            id,
        })
    }

    pub fn read_asset(&self, id: &str, rel_path: &str) -> Result<PreviewAsset, String> {
        let preview = self.preview(id)?;
        let path = preview.resolve_asset_path(rel_path)?;
        let content = preview
            .files
            .get(&path)
            .cloned()
            .ok_or_else(|| format!("preview asset not found: {path}"))?;
        Ok(PreviewAsset {
            content,
            content_type: content_type_for_path(&path).to_string(),
        })
    }

    pub fn read_asset_json(&self, id: &str, rel_path: &str) -> Result<String, String> {
        Ok(self.read_asset(id, rel_path)?.serialize_json())
    }

    pub fn invoke_backend(
        &mut self,
        id: &str,
        verb: &str,
        args: &[String],
    ) -> Result<String, String> {
        let preview = self.preview_mut(id)?;
        if verb.trim().is_empty() {
            return Err("preview verb must not be empty".to_string());
        }
        let mut input = Vec::with_capacity(args.len() + 1);
        input.push(verb.to_string());
        input.extend(args.iter().cloned());
        let result =
            run_memory_backend(&preview.id, &input, &preview.bundle, preview.state.clone())
                .map_err(|e| e.to_string())?;
        fold_records_in_memory(&mut preview.state, &result.records).map_err(|e| e.to_string())?;
        Ok(result.output)
    }

    fn preview(&self, id: &str) -> Result<&Preview, String> {
        self.previews
            .get(id)
            .ok_or_else(|| format!("no such preview: {id}"))
    }

    fn preview_mut(&mut self, id: &str) -> Result<&mut Preview, String> {
        self.previews
            .get_mut(id)
            .ok_or_else(|| format!("no such preview: {id}"))
    }
}

impl Preview {
    fn resolve_asset_path(&self, rel_path: &str) -> Result<String, String> {
        if rel_path.is_empty() {
            return Ok(self.ui.clone());
        }
        let rel = normalize_rel_path(rel_path)?;
        Ok(match self.ui.rsplit_once('/') {
            Some((parent, _)) if !parent.is_empty() => format!("{parent}/{rel}"),
            _ => rel,
        })
    }
}

fn parse_files_payload(raw: &str) -> Result<Vec<PreviewFile>, String> {
    if raw.trim_start().starts_with('[') {
        Vec::<PreviewFile>::deserialize_json(raw).map_err(|e| format!("preview files JSON: {e}"))
    } else {
        PreviewFilesPayload::deserialize_json(raw)
            .map(|p| p.files)
            .map_err(|e| format!("preview files JSON: {e}"))
    }
}

fn validate_files(files: Vec<PreviewFile>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for file in files {
        let path = normalize_rel_path(&file.path)?;
        let ext = extension(&path).ok_or_else(|| format!("unsupported file extension: {path}"))?;
        if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!("unsupported file extension: {path}"));
        }
        if out.insert(path.clone(), file.content).is_some() {
            return Err(format!("duplicate preview file: {path}"));
        }
    }
    Ok(out)
}

fn parse_manifest(text: &str) -> Result<BundleManifest, String> {
    let m = PreviewManifest::deserialize_json(text).map_err(|e| format!("manifest.json: {e}"))?;
    if m.backend.trim().is_empty() {
        return Err("missing manifest.backend".to_string());
    }
    if m.ui.trim().is_empty() {
        return Err("missing manifest.ui".to_string());
    }
    Ok(BundleManifest {
        id: m.id,
        name: m.name,
        backend: m.backend,
        ui: m.ui,
        resources: m.resources,
    })
}

fn validate_manifest_id(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing manifest.id".to_string());
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!(
            "manifest.id is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn normalize_rel_path(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("path must not be empty".to_string());
    }
    if input.contains('\\') {
        return Err(format!("path must use '/' separators: {input}"));
    }
    let mut parts = Vec::new();
    for component in Path::new(input).components() {
        match component {
            Component::Normal(part) => {
                let s = part
                    .to_str()
                    .ok_or_else(|| format!("path is not valid UTF-8: {input}"))?;
                if !s.is_empty() {
                    parts.push(s.to_string());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("parent-dir components are not allowed: {input}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("absolute paths are not allowed: {input}"));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("path must name a file: {input}"));
    }
    Ok(parts.join("/"))
}

fn extension(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let (_, ext) = file.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

fn content_type_for_path(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
