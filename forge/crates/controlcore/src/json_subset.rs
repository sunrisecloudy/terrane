//! `jsonMatchesSubset` — bridge-call and core-action assertion matcher.

use crate::canonical_json::canonical_json;
use serde_json::Value;

/// True when `actual` contains every field in `expected`, recursively.
///
/// Objects match by subset; arrays and scalars require canonical JSON equality.
pub fn json_matches_subset(actual: &Value, expected: &Value) -> bool {
    if expected.is_null() {
        return actual.is_null();
    }
    if let Some(expected_object) = expected.as_object() {
        let Some(actual_object) = actual.as_object() else {
            return false;
        };
        return expected_object.iter().all(|(key, expected_value)| {
            actual_object
                .get(key)
                .is_some_and(|actual_value| json_matches_subset(actual_value, expected_value))
        });
    }
    canonical_json(actual) == canonical_json(expected)
}

/// Evaluate a subset match from a control command payload.
pub fn json_matches_subset_from_payload(payload: &Value) -> forge_domain::Result<Value> {
    let actual = payload.get("actual").ok_or_else(|| {
        forge_domain::CoreError::ValidationError(
            "control.json_matches_subset requires actual".into(),
        )
    })?;
    let expected = payload.get("expected").ok_or_else(|| {
        forge_domain::CoreError::ValidationError(
            "control.json_matches_subset requires expected".into(),
        )
    })?;
    let matches = json_matches_subset(actual, expected);
    Ok(serde_json::json!({ "ok": matches, "matches": matches }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_subset_matches() {
        let actual = json!({"type": "Toast", "message": "hello", "level": "success"});
        let expected = json!({"type": "Toast", "message": "hello"});
        assert!(json_matches_subset(&actual, &expected));
    }

    #[test]
    fn missing_key_fails() {
        let actual = json!({"type": "Toast"});
        let expected = json!({"message": "hello"});
        assert!(!json_matches_subset(&actual, &expected));
    }

    #[test]
    fn null_requires_null() {
        assert!(json_matches_subset(&Value::Null, &Value::Null));
        assert!(!json_matches_subset(&json!("x"), &Value::Null));
    }
}