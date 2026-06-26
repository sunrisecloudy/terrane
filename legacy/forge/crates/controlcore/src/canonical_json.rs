//! Canonical JSON serialization shared by control-plane matchers.
//!
//! Object keys are sorted lexicographically at every nesting level. Arrays keep
//! caller order except where a specific algorithm reorders them (snapshot storage
//! rows).

use serde_json::{Map, Value};

/// Serialize `value` to a deterministic JSON string with sorted object keys.
pub fn canonical_json(value: &Value) -> String {
    canonical_json_inner(value).to_string()
}

fn canonical_json_inner(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_json_inner(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(canonical_json_inner).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_keys_are_sorted() {
        assert_eq!(
            canonical_json(&json!({"b": 1, "a": 2})),
            r#"{"a":2,"b":1}"#
        );
    }
}