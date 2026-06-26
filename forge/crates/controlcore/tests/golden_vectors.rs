//! Golden vectors for forge-controlcore (forge-core-plan B6).

use forge_controlcore::dispatch;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[derive(serde::Deserialize)]
struct VectorFile {
    vectors: Vec<Vector>,
}

#[derive(serde::Deserialize)]
struct Vector {
    name: String,
    command: String,
    input: Value,
    expect: Value,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("control")
        .join("manifest.json")
}

fn assert_subset(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (_, Value::Null) => {}
        (Value::Object(actual_map), Value::Object(expected_map)) => {
            for (key, expected_value) in expected_map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let actual_value = actual_map
                    .get(key)
                    .unwrap_or_else(|| panic!("missing field {child_path} in actual result"));
                assert_subset(actual_value, expected_value, &child_path);
            }
        }
        (left, right) => {
            assert_eq!(left, right, "mismatch at {path}");
        }
    }
}

#[test]
fn golden_vectors_match_expectations() {
    let text = fs::read_to_string(fixture_path()).expect("read control fixture manifest");
    let file: VectorFile = serde_json::from_str(&text).expect("parse control fixture manifest");

    for vector in file.vectors {
        if vector.name == "package_hashes_deterministic" {
            let result = dispatch(&vector.command, &vector.input).expect(&vector.name);
            assert!(result.get("manifestHash").and_then(Value::as_str).is_some_and(|h| h.starts_with("sha256:")));
            assert!(result.get("contentHash").and_then(Value::as_str).is_some_and(|h| h.starts_with("sha256:")));
            assert!(result.get("permissionsHash").and_then(Value::as_str).is_some_and(|h| h.starts_with("sha256:")));
            assert!(result.get("policyHash").and_then(Value::as_str).is_some_and(|h| h.starts_with("sha256:")));
            let again = dispatch(&vector.command, &vector.input).expect("deterministic replay");
            assert_eq!(result, again, "{}", vector.name);
            continue;
        }

        let result = dispatch(&vector.command, &vector.input).unwrap_or_else(|error| {
            panic!("{} failed: {error}", vector.name);
        });
        assert_subset(&result, &vector.expect, &vector.name);
    }
}

#[test]
fn snapshot_compare_hashes_are_canonical() {
    let left = serde_json::json!({"snapshotId": "a", "value": 1});
    let right = serde_json::json!({"snapshotId": "b", "value": 1});
    let result = dispatch(
        "control.compare_snapshot",
        &serde_json::json!({ "left": left, "right": right }),
    )
    .expect("compare");
    let left_hash = result["leftHash"].as_str().expect("leftHash");
    let right_hash = result["rightHash"].as_str().expect("rightHash");
    assert_eq!(left_hash, right_hash);
    assert!(left_hash.starts_with("sha256:"));
    assert_eq!(left_hash.len(), "sha256:".len() + 64);
}