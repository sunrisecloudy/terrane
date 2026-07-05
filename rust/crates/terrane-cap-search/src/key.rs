//! Reserved KV key layout for the search projection.

use terrane_cap_interface::{Error, Result};

/// Reserved app-local KV prefix for the search capability.
pub const SEARCH_PREFIX: &str = "__terrane/search/v1/";

pub fn config_key() -> String {
    format!("{SEARCH_PREFIX}config")
}

pub fn doc_key(doc_id: &str) -> Result<String> {
    validate_doc_id(doc_id)?;
    Ok(format!("{SEARCH_PREFIX}doc/{doc_id}"))
}

pub fn embedding_key(model: &str, doc_id: &str) -> Result<String> {
    validate_doc_id(doc_id)?;
    validate_model_id(model)?;
    Ok(format!("{SEARCH_PREFIX}embeddings/{model}/{doc_id}"))
}

pub fn doc_prefix() -> String {
    format!("{SEARCH_PREFIX}doc/")
}

pub fn embedding_prefix(model: &str) -> Result<String> {
    validate_model_id(model)?;
    Ok(format!("{SEARCH_PREFIX}embeddings/{model}/"))
}

/// The prefix covering every model's embeddings, for removing a document's
/// vectors regardless of which embedding model produced them.
pub fn embeddings_root() -> String {
    format!("{SEARCH_PREFIX}embeddings/")
}

pub fn validate_doc_id(doc_id: &str) -> Result<()> {
    validate_name(doc_id, "doc_id")
}

pub fn validate_model_id(model: &str) -> Result<()> {
    validate_name(model, "model")
}

fn validate_name(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::InvalidInput(format!("{label} must not be empty")));
    }
    if value.len() > 128 {
        return Err(Error::InvalidInput(format!(
            "{label} must be at most 128 bytes"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(Error::InvalidInput(format!(
            "{label} must be a portable ASCII identifier"
        )));
    }
    Ok(())
}
