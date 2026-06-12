//! Applet/script manifest: capabilities + resource limits.
//!
//! prd-merged/01 CR-3 (capability namespaces), CR-5 (resource limits),
//! prd-merged/07 §07-runtime entrypoint manifest, SC-8 (capability grammar).
//! M0a subset: enough capability surface for the spine demo (db, storage, ui,
//! time, random) plus the limit fields the runtime enforces.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A runnable unit's manifest, stored as a CRDT document (prd-merged/01 CR-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entrypoint: String,
    /// Minimum host API version the code requires (prd-merged/01 CR-11).
    #[serde(default = "default_min_api")]
    pub min_api: String,
    /// Whether the run is deterministic (prd-merged/01 CR-8): time/random come
    /// from recorded/seeded seams and live network is forbidden.
    #[serde(default)]
    pub deterministic: bool,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub limits: Limits,
}

fn default_min_api() -> String {
    "forge-api@0.1".to_string()
}

impl Manifest {
    /// Validate structural invariants (prd-merged/01 CR-A4 ValidationError).
    pub fn validate(&self) -> crate::Result<()> {
        if self.entrypoint.trim().is_empty() {
            return Err(crate::CoreError::ValidationError("manifest.entrypoint is empty".into()));
        }
        if !self.min_api.starts_with("forge-api@") {
            return Err(crate::CoreError::ValidationError(format!(
                "manifest.min_api must be 'forge-api@MAJOR.MINOR', got {:?}",
                self.min_api
            )));
        }
        self.limits.validate()?;
        Ok(())
    }
}

/// Capability grants. Each is action + resource + constraints
/// (prd-merged/07 SC-8). M0a models the spine subset; net/files/secrets/etc.
/// land in later milestones but the shape is here so manifests don't churn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Storage KV scopes the applet may read/write (`ctx.storage`).
    #[serde(default)]
    pub storage: StorageGrant,
    /// Collections the applet may read/write via `ctx.db` (prd-merged/02 DL-18).
    #[serde(default)]
    pub db: DbGrant,
    /// Whether the applet may emit UI trees (`ctx.ui`). Always allowed in M0a.
    #[serde(default = "default_true")]
    pub ui: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Capabilities {
    fn default() -> Self {
        // `ui` defaults to true so an absent `capabilities` object (which serde
        // fills via `Capabilities::default()`) still grants UI in M0a, matching
        // the field-level `#[serde(default = "default_true")]`.
        Capabilities { storage: StorageGrant::default(), db: DbGrant::default(), ui: true }
    }
}

/// Per-applet KV scope. Glob-ish prefixes, e.g. `app/*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StorageGrant {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

/// Collections the applet may touch (named grants; row filters are v1.x).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DbGrant {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

/// Resource limits per instance/run. prd-merged/01 CR-5, prd-merged/07 §07.
/// Exceeding any limit → suspension with `ResourceLimitExceeded`, never a host
/// crash. Accounting lives in the shared host shim, not per-engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Wall-clock budget for a run/turn in milliseconds.
    pub wall_ms: u64,
    /// Interrupt/fuel budget: max engine "ticks" before a cooperative check.
    pub fuel: u64,
    /// Memory ceiling in bytes (mapped to the engine's memory limit).
    pub memory_bytes: u64,
    /// Max number of host (`ctx.*`) calls in a single run (flood guard).
    pub max_host_calls: u64,
    /// Max bytes written to storage in a single run.
    pub storage_bytes: u64,
    /// Max bytes of log output captured per run.
    pub log_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        // Conservative spine defaults; shells may override (prd-merged/01 CR-5).
        Limits {
            wall_ms: 3_000,
            fuel: 10_000_000,
            memory_bytes: 64 * 1024 * 1024,
            max_host_calls: 10_000,
            storage_bytes: 10 * 1024 * 1024,
            log_bytes: 256 * 1024,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> crate::Result<()> {
        let zero_field = [
            ("wall_ms", self.wall_ms),
            ("fuel", self.fuel),
            ("memory_bytes", self.memory_bytes),
            ("max_host_calls", self.max_host_calls),
        ]
        .into_iter()
        .find(|(_, v)| *v == 0);
        if let Some((name, _)) = zero_field {
            return Err(crate::CoreError::ValidationError(format!(
                "limits.{name} must be > 0"
            )));
        }
        Ok(())
    }
}

/// Free-form extension slot preserved across versions (forward-compat habit,
/// mirrors prd-merged/02 DL-9 unknown-field preservation at the manifest level).
pub type Extensions = BTreeMap<String, serde_json::Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_manifest_validates() {
        let m = Manifest {
            entrypoint: "src/main.ts".into(),
            min_api: default_min_api(),
            deterministic: true,
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        };
        assert!(m.validate().is_ok());
    }

    #[test]
    fn empty_entrypoint_is_rejected() {
        let m = Manifest {
            entrypoint: "  ".into(),
            min_api: default_min_api(),
            deterministic: true,
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        };
        assert_eq!(m.validate().unwrap_err().code(), "ValidationError");
    }

    #[test]
    fn zero_fuel_is_rejected() {
        let l = Limits {
            fuel: 0,
            ..Default::default()
        };
        assert_eq!(l.validate().unwrap_err().code(), "ValidationError");
    }

    #[test]
    fn bad_min_api_is_rejected() {
        let m = Manifest {
            entrypoint: "src/main.ts".into(),
            min_api: "1.0".into(),
            deterministic: true,
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        };
        assert!(m.validate().is_err());
    }

    #[test]
    fn manifest_deserializes_with_defaults() {
        // Only entrypoint provided; everything else defaults.
        let m: Manifest = serde_json::from_str(r#"{"entrypoint":"src/main.ts"}"#).unwrap();
        assert_eq!(m.min_api, "forge-api@0.1");
        assert!(m.capabilities.ui);
        assert_eq!(m.limits, Limits::default());
        assert!(m.validate().is_ok());
    }
}
