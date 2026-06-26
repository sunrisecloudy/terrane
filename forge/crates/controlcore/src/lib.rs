//! forge-controlcore: pure JSON-in/JSON-out DevControlPlane algorithms.
//!
//! Debug-only surface consumed through `control.*` commands on the existing
//! `handle_command` seam (forge-core-plan Q1). No platform I/O — shells fetch
//! rows / files and pass JSON payloads here.

mod backup;
mod canonical_json;
mod json_subset;
mod package;
mod signing;
mod snapshot_compare;

pub use backup::{
    backup_content_hash, backup_content_hash_from_payload, backup_validate_from_payload,
    validate_backup_document,
};
pub use canonical_json::canonical_json;
pub use json_subset::{json_matches_subset, json_matches_subset_from_payload};
pub use package::{
    package_hashes, package_hashes_from_payload, validate_package, validate_package_from_payload,
    PackageFile,
};
pub use signing::{
    generate_token_from_payload, sign_payload_from_payload, verify_signature_from_payload,
};
pub use snapshot_compare::{
    compare_snapshots, compare_snapshots_from_payload, comparable_snapshot,
};

use forge_domain::{CoreError, Result};
use serde_json::Value;

/// Dispatch a `control.*` command name + payload to the pure handler.
pub fn dispatch(name: &str, payload: &Value) -> Result<Value> {
    match name {
        "control.compare_snapshot" => compare_snapshots_from_payload(payload),
        "control.json_matches_subset" => json_matches_subset_from_payload(payload),
        "control.package_validate" => package::validate_package_from_payload(payload),
        "control.package_hashes" => package::package_hashes_from_payload(payload),
        "control.backup_validate" => backup_validate_from_payload(payload),
        "control.backup_content_hash" => backup_content_hash_from_payload(payload),
        "control.generate_token" => signing::generate_token_from_payload(payload),
        "control.sign_payload" => signing::sign_payload_from_payload(payload),
        "control.verify_signature" => signing::verify_signature_from_payload(payload),
        other => Err(CoreError::ValidationError(format!(
            "unknown control command {other:?}"
        ))),
    }
}