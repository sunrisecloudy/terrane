//! Runtime snapshot compare with volatile-field stripping and storage-row normalization.

use crate::canonical_json::canonical_json;
use forge_domain::{content_hash, CoreError, Result};
use serde_json::{Map, Value};

const SKIP_FIELDS: &[&str] = &["createdAt", "snapshotId", "updated_at", "updatedAt"];

/// Normalize a snapshot for comparison: strip volatile fields, rename `appStorage`
/// to `storage`, and sort storage rows by `app_id|key`.
pub fn comparable_snapshot(value: &Value) -> Value {
    comparable_value(value, false)
}

/// Compare two snapshots after normalization.
pub fn compare_snapshots(left: &Value, right: &Value) -> Result<Value> {
    let left_comparable = comparable_snapshot(left);
    let right_comparable = comparable_snapshot(right);
    let left_json = canonical_json(&left_comparable);
    let right_json = canonical_json(&right_comparable);
    let equal = left_json == right_json;
    Ok(serde_json::json!({
        "ok": equal,
        "equal": equal,
        "leftHash": content_hash(left_json.as_bytes()),
        "rightHash": content_hash(right_json.as_bytes()),
    }))
}

/// Compare snapshots supplied in a control command payload.
pub fn compare_snapshots_from_payload(payload: &Value) -> Result<Value> {
    let left = payload.get("left").ok_or_else(|| {
        CoreError::ValidationError(
            "control.compare_snapshot requires left or leftSnapshotId".into(),
        )
    })?;
    let right = payload.get("right").ok_or_else(|| {
        CoreError::ValidationError(
            "control.compare_snapshot requires right or rightSnapshotId".into(),
        )
    })?;
    compare_snapshots(left, right)
}

fn comparable_value(value: &Value, storage_context: bool) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
        Value::Array(items) => {
            let normalized: Vec<Value> = items
                .iter()
                .map(|v| comparable_value(v, storage_context))
                .collect();
            if storage_context {
                let mut sorted = normalized;
                sorted.sort_by_key(storage_sort_key);
                Value::Array(sorted)
            } else {
                Value::Array(normalized)
            }
        }
        Value::Object(map) => {
            if storage_context {
                return Value::Object(normalize_storage_row(map));
            }
            let mut out = Map::new();
            for (key, child) in map {
                if SKIP_FIELDS.contains(&key.as_str()) {
                    continue;
                }
                let normalized_key = if key == "appStorage" { "storage" } else { key.as_str() };
                if normalized_key == "storage" && child.is_array() {
                    out.insert(
                        normalized_key.to_string(),
                        comparable_value(child, true),
                    );
                } else if out.contains_key(normalized_key) && normalized_key == "storage" {
                    // Prefer explicit `storage` over legacy `appStorage`.
                    continue;
                } else {
                    out.insert(
                        normalized_key.to_string(),
                        comparable_value(child, false),
                    );
                }
            }
            Value::Object(out)
        }
    }
}

fn normalize_storage_row(map: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(app_id) = map
        .get("app_id")
        .or_else(|| map.get("appId"))
        .cloned()
    {
        out.insert("app_id".to_string(), app_id);
    }
    if let Some(key) = map.get("key").cloned() {
        out.insert("key".to_string(), key);
    }
    if let Some(value_json) = map
        .get("value_json")
        .or_else(|| map.get("valueJson"))
        .or_else(|| map.get("value"))
        .cloned()
    {
        out.insert("value_json".to_string(), value_json);
    }
    out
}

fn storage_sort_key(row: &Value) -> String {
    let Some(map) = row.as_object() else {
        return String::new();
    };
    let app_id = map
        .get("app_id")
        .or_else(|| map.get("appId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let key = map.get("key").and_then(Value::as_str).unwrap_or_default();
    format!("{app_id}|{key}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_volatile_fields_and_sorts_storage_rows() {
        let left = json!({
            "snapshotId": "snap-a",
            "createdAt": "2026-01-01T00:00:00Z",
            "storage": [
                {"app_id": "notes-lite", "key": "notes-lite:b", "value_json": "[]", "updated_at": "t2"},
                {"app_id": "notes-lite", "key": "notes-lite:a", "value_json": "[]", "updatedAt": "t1"}
            ]
        });
        let right = json!({
            "snapshotId": "snap-b",
            "createdAt": "2026-01-02T00:00:00Z",
            "storage": [
                {"appId": "notes-lite", "key": "notes-lite:a", "value_json": "[]"},
                {"appId": "notes-lite", "key": "notes-lite:b", "value_json": "[]"}
            ]
        });
        let result = compare_snapshots(&left, &right).expect("compare");
        assert_eq!(result["equal"], true);
        assert_eq!(result["ok"], true);
    }
}