use std::collections::BTreeMap;
use std::path::{Component, Path};

use nanoserde::DeJson;
use terrane_cap_interface::{extract_json_object, Error, Result};

use super::BuilderFile;

const SUPPORTED_EXTENSIONS: &[&str] = &["html", "htm", "css", "js", "mjs", "json", "svg"];
const MAX_FILES: usize = 48;
const MAX_TOTAL_BYTES: usize = 512 * 1024;

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
    runtime: String,
    #[nserde(default)]
    backend: String,
    #[nserde(default)]
    ui: String,
    #[nserde(default)]
    resources: Vec<String>,
}

pub fn parse_generated_files(raw: &str, app_id: &str, name: &str) -> Result<Vec<BuilderFile>> {
    let json = extract_json_object(raw, "builder output")?;
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
    if manifest.runtime.trim() != "js" {
        return Err(Error::InvalidInput(format!(
            "builder manifest runtime {:?} must be \"js\"",
            manifest.runtime
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

pub fn validate_id(raw: &str, label: &str) -> Result<String> {
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

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
