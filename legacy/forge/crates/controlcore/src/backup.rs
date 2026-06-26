//! Backup export/import document format validation and content hashing.

use crate::canonical_json::canonical_json;
use forge_domain::{content_hash, CoreError, Result};
use serde_json::{json, Value};

const ALLOWED_TYPES: &[&str] = &["backup", "debug-bundle", "test-fixture"];
const REQUIRED_ARRAYS: &[&str] = &[
    "apps",
    "appVersions",
    "appFiles",
    "appPermissions",
    "appStorage",
];

/// Validate a backup/debug-bundle document shape.
pub fn validate_backup_document(document: &Value) -> Value {
    let mut errors: Vec<Value> = Vec::new();
    let doc_type = document.get("type").and_then(Value::as_str).unwrap_or_default();
    if !ALLOWED_TYPES.contains(&doc_type) {
        errors.push(issue(
            "invalid_backup_type",
            "Backup document type must be backup, debug-bundle, or test-fixture",
            json!({ "type": document.get("type").cloned().unwrap_or(Value::Null) }),
        ));
    }
    for field in REQUIRED_ARRAYS {
        if !document.get(field).is_some_and(Value::is_array) {
            errors.push(issue(
                "missing_backup_section",
                &format!("Backup document requires {field} array"),
                json!({ "field": field }),
            ));
        }
    }
    let ok = errors.is_empty();
    json!({
        "ok": ok,
        "type": document.get("type").cloned().unwrap_or(Value::Null),
        "errors": errors,
    })
}

/// Compute the canonical `contentHash` for a backup document (hash excludes any
/// existing `contentHash` field, matching native export semantics).
pub fn backup_content_hash(document: &Value) -> String {
    let mut copy = document.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("contentHash");
    }
    content_hash(canonical_json(&copy).as_bytes())
}

/// Validate and compute content hash from a control payload.
pub fn backup_validate_from_payload(payload: &Value) -> Result<Value> {
    let document = payload.get("document").ok_or_else(|| {
        CoreError::ValidationError("control.backup_validate requires document".into())
    })?;
    let mut result = validate_backup_document(document);
    if result["ok"].as_bool() == Some(true) {
        if let Some(map) = result.as_object_mut() {
            map.insert(
                "contentHash".to_string(),
                Value::String(backup_content_hash(document)),
            );
        }
    }
    Ok(result)
}

/// Compute backup content hash from a control payload.
pub fn backup_content_hash_from_payload(payload: &Value) -> Result<Value> {
    let document = payload.get("document").ok_or_else(|| {
        CoreError::ValidationError("control.backup_content_hash requires document".into())
    })?;
    Ok(json!({
        "contentHash": backup_content_hash(document),
    }))
}

fn issue(code: &str, message: &str, details: Value) -> Value {
    json!({
        "code": code,
        "message": message,
        "details": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_missing_sections() {
        let result = validate_backup_document(&json!({"type": "backup"}));
        assert_eq!(result["ok"], false);
    }

    #[test]
    fn content_hash_ignores_existing_field() {
        let doc = json!({
            "type": "backup",
            "apps": [],
            "appVersions": [],
            "appFiles": [],
            "appPermissions": [],
            "appStorage": [],
            "contentHash": "sha256:deadbeef"
        });
        let hash = backup_content_hash(&doc);
        assert!(hash.starts_with("sha256:"));
        assert_ne!(hash, "sha256:deadbeef");
    }
}