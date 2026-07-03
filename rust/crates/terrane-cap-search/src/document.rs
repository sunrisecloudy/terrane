use serde::{Deserialize, Serialize};
use serde_json::Value;
use terrane_cap_interface::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchDocument {
    pub text: String,
    #[serde(default)]
    pub metadata: Value,
}

pub fn parse_document(raw: &str) -> Result<SearchDocument> {
    let doc: SearchDocument = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("document JSON is invalid: {e}")))?;
    if doc.text.trim().is_empty() {
        return Err(Error::InvalidInput("document text must not be empty".into()));
    }
    Ok(doc)
}

pub fn canonical_document_json(doc: &SearchDocument) -> Result<String> {
    serde_json::to_string(doc).map_err(|e| Error::Storage(format!("serialize document: {e}")))
}

pub fn doc_id_from_key(key: &str, prefix: &str) -> Option<String> {
    key.strip_prefix(prefix).map(str::to_string)
}