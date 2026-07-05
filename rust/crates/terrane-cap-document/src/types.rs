use std::collections::BTreeMap;

use serde_json::{Map, Value};
use terrane_cap_interface::{state_ref, AppId, Error, Result, StateStore};

pub const MAX_BODY_BYTES: usize = 1_048_576;
pub const MAX_METADATA_BYTES: usize = 16_384;
pub const MAX_TITLE_CHARS: usize = 256;
pub const MAX_DOCUMENTS_PER_APP: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: String,
    pub title: String,
    pub body: String,
    pub metadata_json: String,
    pub updated_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentState {
    pub docs: BTreeMap<AppId, BTreeMap<String, Document>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub metadata_patch_json: Option<String>,
}

impl DocumentPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.body.is_none() && self.metadata_patch_json.is_none()
    }
}

pub fn validate_document_id(id: &str) -> Result<()> {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return Err(Error::InvalidInput(format!(
            "invalid document id {id:?}: expected 1-128 chars matching ^[A-Za-z0-9][A-Za-z0-9_-]{{0,127}}$"
        )));
    }
    if !bytes[0].is_ascii_alphanumeric()
        || !bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(Error::InvalidInput(format!(
            "invalid document id {id:?}: expected ^[A-Za-z0-9][A-Za-z0-9_-]{{0,127}}$"
        )));
    }
    Ok(())
}

pub fn validate_title(title: &str) -> Result<()> {
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(Error::InvalidInput(format!(
            "document title exceeds {MAX_TITLE_CHARS} chars"
        )));
    }
    Ok(())
}

pub fn validate_body(body: &str) -> Result<()> {
    if body.len() > MAX_BODY_BYTES {
        return Err(Error::InvalidInput(format!(
            "document body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok(())
}

pub fn parse_metadata_json(raw: Option<&str>) -> Result<String> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => canonical_object_json(raw, "metadataJson"),
        _ => Ok("{}".to_string()),
    }
}

pub fn parse_patch_json(raw: &str) -> Result<DocumentPatch> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("patchJson is invalid JSON: {e}")))?;
    let Value::Object(mut object) = value else {
        return Err(Error::InvalidInput(
            "patchJson must be a JSON object".into(),
        ));
    };
    let title = optional_string(&mut object, "title")?;
    let body = optional_string(&mut object, "body")?;
    let metadata_patch_json = match object.remove("metadata") {
        Some(Value::Object(map)) => Some(canonical_metadata_value(Value::Object(map))?),
        Some(_) => {
            return Err(Error::InvalidInput(
                "patchJson.metadata must be a JSON object".into(),
            ))
        }
        None => None,
    };
    if !object.is_empty() {
        let keys = object.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(Error::InvalidInput(format!(
            "patchJson contains unsupported fields: {keys}"
        )));
    }
    if let Some(title) = &title {
        validate_title(title)?;
    }
    if let Some(body) = &body {
        validate_body(body)?;
    }
    Ok(DocumentPatch {
        title,
        body,
        metadata_patch_json,
    })
}

pub fn apply_metadata_patch(current_json: &str, patch_json: &str) -> Result<String> {
    let mut current = parse_json_object(current_json, "stored metadata")?;
    let patch = parse_json_object(patch_json, "metadata patch")?;
    merge_patch(&mut current, &patch);
    canonical_metadata_value(Value::Object(current))
}

pub fn document_json(document: &Document) -> Result<String> {
    let metadata = serde_json::from_str::<Value>(&document.metadata_json)
        .map_err(|e| Error::Storage(format!("stored document metadata is invalid: {e}")))?;
    let value = serde_json::json!({
        "id": document.id,
        "title": document.title,
        "body": document.body,
        "metadata": metadata,
        "updatedSeq": document.updated_seq,
    });
    serde_json::to_string(&value).map_err(|e| Error::Storage(format!("serialize document: {e}")))
}

pub fn document_list_json(state: &dyn StateStore, app: &str) -> Result<String> {
    let rows = state_ref::<DocumentState>(state, "document")?
        .docs
        .get(app)
        .map(|docs| {
            docs.values()
                .map(|doc| {
                    serde_json::json!({
                        "id": doc.id,
                        "title": doc.title,
                        "bodyBytes": doc.body.len(),
                        "updatedSeq": doc.updated_seq,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::to_string(&rows)
        .map_err(|e| Error::Storage(format!("serialize document list: {e}")))
}

pub fn get_document_json(state: &dyn StateStore, app: &str, id: &str) -> Result<Option<String>> {
    validate_document_id(id)?;
    let Some(document) = state_ref::<DocumentState>(state, "document")?
        .docs
        .get(app)
        .and_then(|docs| docs.get(id))
    else {
        return Ok(None);
    };
    Ok(Some(document_json(document)?))
}

pub fn export_markdown(state: &dyn StateStore, app: &str, id: &str) -> Result<String> {
    validate_document_id(id)?;
    state_ref::<DocumentState>(state, "document")?
        .docs
        .get(app)
        .and_then(|docs| docs.get(id))
        .map(|document| document.body.clone())
        .ok_or_else(|| Error::InvalidInput(format!("missing document: {app}/{id}")))
}

pub fn enforce_document_quota(state: &DocumentState, app: &str, id: &str) -> Result<()> {
    let count = state.docs.get(app).map(BTreeMap::len).unwrap_or(0);
    let exists = state
        .docs
        .get(app)
        .is_some_and(|docs| docs.contains_key(id));
    if !exists && count >= MAX_DOCUMENTS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "document quota exceeded for app {app}: maxDocumentsPerApp={MAX_DOCUMENTS_PER_APP}"
        )));
    }
    Ok(())
}

pub fn validate_metadata_size(metadata_json: &str) -> Result<()> {
    if metadata_json.len() > MAX_METADATA_BYTES {
        return Err(Error::InvalidInput(format!(
            "document metadata exceeds {MAX_METADATA_BYTES} bytes"
        )));
    }
    Ok(())
}

fn optional_string(object: &mut Map<String, Value>, key: &str) -> Result<Option<String>> {
    match object.remove(key) {
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(Error::InvalidInput(format!(
            "patchJson.{key} must be a string"
        ))),
        None => Ok(None),
    }
}

fn canonical_object_json(raw: &str, label: &str) -> Result<String> {
    let object = parse_json_object(raw, label)?;
    canonical_metadata_value(Value::Object(object))
}

fn parse_json_object(raw: &str, label: &str) -> Result<Map<String, Value>> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("{label} is invalid JSON: {e}")))?;
    let Value::Object(object) = value else {
        return Err(Error::InvalidInput(format!("{label} must be a JSON object")));
    };
    Ok(object)
}

fn canonical_metadata_value(value: Value) -> Result<String> {
    let raw = serde_json::to_string(&value)
        .map_err(|e| Error::Storage(format!("serialize document metadata: {e}")))?;
    validate_metadata_size(&raw)?;
    Ok(raw)
}

fn merge_patch(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
}
