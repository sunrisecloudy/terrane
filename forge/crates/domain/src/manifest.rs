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
    /// Network egress allowlist for `ctx.net.fetch` (prd-merged/07 SC-8 `net`
    /// namespace, SC-5 egress policy). Default empty → **no network**: an applet
    /// that lists no `net` rules cannot reach the network at all. Each rule is a
    /// scheme://host/path-glob grant plus optional size/timeout/content-type
    /// constraints; there are **no wildcard hosts** in v1 (a host must be an
    /// exact literal). forge-policy's `NetPolicy` evaluates a request against
    /// this list.
    #[serde(default)]
    pub net: NetGrant,
}

fn default_true() -> bool {
    true
}

impl Default for Capabilities {
    fn default() -> Self {
        // `ui` defaults to true so an absent `capabilities` object (which serde
        // fills via `Capabilities::default()`) still grants UI in M0a, matching
        // the field-level `#[serde(default = "default_true")]`. `net` defaults to
        // empty (no network) so an absent `capabilities` object grants zero net.
        Capabilities {
            storage: StorageGrant::default(),
            db: DbGrant::default(),
            ui: true,
            net: NetGrant::default(),
        }
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

/// The applet's network egress allowlist (`ctx.net.fetch`).
///
/// prd-merged/07 SC-8 `net` namespace + SC-5 egress policy. A newtype over a
/// `Vec<NetRule>` so it stays serde-transparent (a manifest writes
/// `"net": [ {…}, {…} ]`) while giving the type a name the rest of the codebase
/// can refer to. **Empty = no network**, the default for every applet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct NetGrant(pub Vec<NetRule>);

impl NetGrant {
    /// Whether the applet declared *any* net rule. An empty grant means the
    /// applet never requested the `net` capability at all — forge-policy maps a
    /// request against an empty grant to `CapabilityRequired`, distinct from a
    /// request that matches no rule of a non-empty grant (`PermissionDenied`).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The rules, in declaration order.
    pub fn rules(&self) -> &[NetRule] {
        &self.0
    }
}

/// One network egress grant (prd-merged/07 SC-8 net grammar, SC-5 constraints).
///
/// `method` + `url` (a `scheme://host/path-glob`) are the action+resource; the
/// remaining fields are the SC-5 constraints forge-policy enforces. Fields use
/// the same names as the T011 `fixtures/network/*` `allowlist` entries so a
/// fixture's allowlist deserializes straight into `Vec<NetRule>`.
///
/// Unknown constraint fields are tolerated for forward-compat (a future
/// constraint added to a fixture/manifest won't fail to parse); the policy
/// engine only acts on the fields it knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetRule {
    /// HTTP method this rule grants, e.g. `GET`, `POST`. Matched
    /// case-insensitively by the policy engine.
    pub method: String,
    /// `scheme://host/path-glob` resource pattern. The host is an **exact
    /// literal** (no wildcard hosts in v1, SC-8); the path may end in a trailing
    /// `*` glob. `https` is required unless the pattern itself is `http://`.
    pub url: String,
    /// Max bytes the response body may be (SC-5 response budget). `None` = the
    /// policy engine applies no rule-level cap (a host/runtime default may
    /// still apply outside policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    /// Max bytes the request body may be (SC-5 request budget). `None` = no
    /// rule-level cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<u64>,
    /// Max request timeout in milliseconds the rule permits (SC-5). A request
    /// asking for a longer timeout than this is denied. `None` = no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Allowed request `Content-Type` values. Empty = unconstrained; non-empty =
    /// a request carrying a content-type not in this set is denied (SC-5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_content_types: Vec<String>,
    /// Allowed response `Content-Type` values. Empty = unconstrained; non-empty =
    /// a response whose content-type is not in this set is denied (SC-5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_content_types: Vec<String>,
    /// Header names into which a secret ref may be injected for this destination
    /// (prd-merged/07 secrets `injectInto`/`netHosts`). A secret-bearing header
    /// is only permitted if its name is listed here *and* the request targets
    /// this rule's host; otherwise the request is denied. Empty = no secret
    /// header may be attached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_secret_headers: Vec<String>,
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

    // --- Net capability allowlist (SC-8) ------------------------------------

    #[test]
    fn net_grant_defaults_to_empty_no_network() {
        // An absent `capabilities` (and an absent `net`) grants zero network.
        let m: Manifest = serde_json::from_str(r#"{"entrypoint":"src/main.ts"}"#).unwrap();
        assert!(m.capabilities.net.is_empty());
        assert!(m.capabilities.net.rules().is_empty());
    }

    #[test]
    fn net_rule_deserializes_from_fixture_shaped_allowlist() {
        // The same JSON shape the T011 fixtures put under "allowlist": a method,
        // a scheme://host/path-glob url, and the SC-5 constraint fields.
        let json = r#"{
            "method": "GET",
            "url": "https://api.example.com/public/*",
            "max_response_bytes": 1048576,
            "timeout_ms": 2000,
            "response_content_types": ["application/json"]
        }"#;
        let rule: NetRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.method, "GET");
        assert_eq!(rule.url, "https://api.example.com/public/*");
        assert_eq!(rule.max_response_bytes, Some(1_048_576));
        assert_eq!(rule.timeout_ms, Some(2000));
        assert_eq!(rule.response_content_types, vec!["application/json".to_string()]);
        // Unset constraints default cleanly.
        assert_eq!(rule.max_body_bytes, None);
        assert!(rule.request_content_types.is_empty());
        assert!(rule.allow_secret_headers.is_empty());
    }

    #[test]
    fn net_grant_is_a_transparent_array_in_capabilities() {
        // A manifest writes `"net": [ {…} ]` directly under capabilities.
        let json = r#"{
            "entrypoint": "src/main.ts",
            "capabilities": {
                "net": [
                    { "method": "POST", "url": "https://api.example.com/forms/*", "max_body_bytes": 4096,
                      "request_content_types": ["application/json"] }
                ]
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        let rules = m.capabilities.net.rules();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].method, "POST");
        assert_eq!(rules[0].max_body_bytes, Some(4096));
        assert_eq!(rules[0].request_content_types, vec!["application/json".to_string()]);
        // A `net` grant does not disturb the M0a ui-default.
        assert!(m.capabilities.ui);
    }

    #[test]
    fn net_rule_with_secret_headers_roundtrips() {
        let rule = NetRule {
            method: "GET".into(),
            url: "https://api.example.com/private/*".into(),
            allow_secret_headers: vec!["Authorization".into()],
            ..Default::default()
        };
        let s = serde_json::to_string(&rule).unwrap();
        let back: NetRule = serde_json::from_str(&s).unwrap();
        assert_eq!(rule, back);
        // Optional/empty constraints are omitted from the wire form (clean JSON).
        assert!(!s.contains("max_response_bytes"), "unset cap omitted: {s}");
        assert!(s.contains("allow_secret_headers"), "set field present: {s}");
    }
}
