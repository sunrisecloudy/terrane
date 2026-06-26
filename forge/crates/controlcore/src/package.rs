//! Package validate/hash — SHA256 over manifest, files, permissions, and policy.

use crate::canonical_json::canonical_json;
use forge_domain::{content_hash, CoreError, Result};
use serde_json::{json, Value};

const MAX_PACKAGE_FILES: usize = 32;
const MAX_MIGRATION_FILES: usize = 16;

const REQUIRED_FILES: &[&str] = &["manifest.json", "index.html", "styles.css", "app.js"];
const OPTIONAL_FILES: &[&str] = &["smoke-tests.json", "README.md"];
const REQUIRED_MANIFEST_FIELDS: &[&str] = &[
    "id",
    "name",
    "version",
    "runtimeVersion",
    "dataVersion",
    "entry",
    "description",
    "permissions",
    "storagePrefix",
    "capabilities",
    "resourceBudget",
    "networkPolicy",
];

/// Compute canonical package hashes from manifest JSON and file records.
pub fn package_hashes(manifest: &Value, files: &[PackageFile]) -> Value {
    let manifest_hash = content_hash(canonical_json(manifest).as_bytes());
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|left, right| left.path.cmp(&right.path));
    let content_body = sorted_files
        .iter()
        .map(|file| format!("{}\n{}\n", file.path, file.content_hash))
        .collect::<String>();
    let permissions = manifest
        .get("permissions")
        .and_then(Value::as_array)
        .map(|items| {
            let mut values: Vec<&Value> = items.iter().collect();
            values.sort_by_key(|value| canonical_json(value));
            Value::Array(values.into_iter().cloned().collect())
        })
        .unwrap_or_else(|| Value::Array(vec![]));
    let policy = json!({
        "capabilities": manifest.get("capabilities").cloned().unwrap_or(json!({})),
        "networkPolicy": manifest.get("networkPolicy").cloned().unwrap_or(json!({})),
        "resourceBudget": manifest.get("resourceBudget").cloned().unwrap_or(json!({})),
    });
    json!({
        "manifestHash": manifest_hash,
        "contentHash": content_hash(content_body.as_bytes()),
        "permissionsHash": content_hash(canonical_json(&permissions).as_bytes()),
        "policyHash": content_hash(canonical_json(&policy).as_bytes()),
    })
}

/// Validate a package described as JSON (`manifest` + `files` array).
pub fn validate_package(manifest: &Value, files: &[PackageFile]) -> Value {
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();

    if files.len() > MAX_PACKAGE_FILES {
        errors.push(issue(
            "resource_budget_exceeded",
            "Package exceeds hard file count cap",
            json!({ "files": files.len(), "maxFiles": MAX_PACKAGE_FILES }),
        ));
    }
    let migration_count = files
        .iter()
        .filter(|file| file.path.starts_with("migrations/"))
        .count();
    if migration_count > MAX_MIGRATION_FILES {
        errors.push(issue(
            "resource_budget_exceeded",
            "Package exceeds hard migration file count cap",
            json!({ "migrations": migration_count, "maxMigrations": MAX_MIGRATION_FILES }),
        ));
    }

    for file in files {
        if file.path.starts_with("assets/") {
            errors.push(issue(
                "unexpected_package_path",
                "Package contains an unexpected path",
                json!({ "path": file.path }),
            ));
            continue;
        }
        let allowed = REQUIRED_FILES.contains(&file.path.as_str())
            || OPTIONAL_FILES.contains(&file.path.as_str())
            || file.path.starts_with("migrations/");
        if !allowed {
            errors.push(issue(
                "unexpected_package_path",
                "Package contains an unexpected path",
                json!({ "path": file.path }),
            ));
        }
    }

    for required in REQUIRED_FILES {
        if !files.iter().any(|file| file.path == *required) {
            errors.push(issue(
                "missing_required_file",
                &format!("{required} is required"),
                json!({ "path": required }),
            ));
        }
    }

    if manifest.as_object().is_none_or(|map| map.is_empty()) {
        errors.push(issue(
            "invalid_manifest_json",
            "manifest.json must parse as JSON",
            json!({}),
        ));
    } else {
        for field in REQUIRED_MANIFEST_FIELDS {
            if manifest.get(field).is_none() {
                errors.push(issue(
                    "missing_manifest_field",
                    &format!("manifest.{field} is required"),
                    json!({ "field": field }),
                ));
            }
        }
        if manifest.get("networkAllowlist").is_some() {
            errors.push(issue(
                "removed_manifest_field",
                "manifest.networkAllowlist was removed; use networkPolicy",
                json!({ "field": "networkAllowlist" }),
            ));
        }
        if let Some(id) = manifest.get("id").and_then(Value::as_str) {
            let valid_id = id
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_lowercase())
                && id.len() >= 3
                && id.len() <= 64
                && id
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
            if !valid_id {
                errors.push(issue(
                    "invalid_manifest_id",
                    "manifest.id must be lowercase kebab-case",
                    json!({ "value": id }),
                ));
            }
            let expected_prefix = format!("{id}:");
            if manifest.get("storagePrefix").and_then(Value::as_str) != Some(expected_prefix.as_str())
            {
                errors.push(issue(
                    "invalid_storage_prefix",
                    "manifest.storagePrefix must equal <id>:",
                    json!({
                        "expected": expected_prefix,
                        "actual": manifest.get("storagePrefix").cloned().unwrap_or(Value::Null)
                    }),
                ));
            }
        }
        if manifest.get("entry").and_then(Value::as_str) != Some("index.html") {
            errors.push(issue(
                "invalid_entry",
                "manifest.entry must be index.html",
                json!({ "value": manifest.get("entry").cloned().unwrap_or(Value::Null) }),
            ));
        }
    }

    if !files.iter().any(|file| file.path == "smoke-tests.json") {
        warnings.push(issue(
            "smoke_tests_missing",
            "Package has no smoke-tests.json",
            json!({}),
        ));
    }

    let ok = errors.is_empty();
    json!({
        "ok": ok,
        "appId": manifest.get("id").cloned().unwrap_or(Value::Null),
        "version": manifest.get("version").cloned().unwrap_or(Value::Null),
        "runtimeVersion": manifest.get("runtimeVersion").cloned().unwrap_or(Value::Null),
        "dataVersion": manifest.get("dataVersion").cloned().unwrap_or(Value::Null),
        "files": files.iter().map(|file| &file.path).collect::<Vec<_>>(),
        "errors": errors,
        "warnings": warnings,
        "hashes": package_hashes(manifest, files),
    })
}

/// Parse package files from a control payload and validate.
pub fn validate_package_from_payload(payload: &Value) -> Result<Value> {
    let manifest = payload.get("manifest").ok_or_else(|| {
        CoreError::ValidationError("control.package_validate requires manifest".into())
    })?;
    let files = parse_files(payload.get("files").ok_or_else(|| {
        CoreError::ValidationError("control.package_validate requires files".into())
    })?)?;
    Ok(validate_package(manifest, &files))
}

/// Parse package files from a control payload and return hashes only.
pub fn package_hashes_from_payload(payload: &Value) -> Result<Value> {
    let manifest = payload.get("manifest").ok_or_else(|| {
        CoreError::ValidationError("control.package_hashes requires manifest".into())
    })?;
    let files = parse_files(payload.get("files").ok_or_else(|| {
        CoreError::ValidationError("control.package_hashes requires files".into())
    })?)?;
    Ok(package_hashes(manifest, &files))
}

#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: String,
    pub content: String,
    pub content_hash: String,
}

fn parse_files(value: &Value) -> Result<Vec<PackageFile>> {
    let Some(items) = value.as_array() else {
        return Err(CoreError::ValidationError(
            "files must be a JSON array".into(),
        ));
    };
    let mut files = Vec::with_capacity(items.len());
    for item in items {
        let path = item
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::ValidationError("each file requires path".into()))?
            .to_string();
        let content = item
            .get("content")
            .or_else(|| item.get("content_text"))
            .or_else(|| item.get("contentText"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let content_hash = item
            .get("contentHash")
            .or_else(|| item.get("content_hash"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| content_hash(content.as_bytes()));
        files.push(PackageFile {
            path,
            content,
            content_hash,
        });
    }
    Ok(files)
}

fn issue(code: &str, message: &str, details: Value) -> Value {
    json!({
        "code": code,
        "message": message,
        "details": details,
    })
}