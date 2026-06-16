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
    /// Marketplace compatibility declaration (prd-merged/08 MP-4 / MP-8): the
    /// `min_app_version` + the `required_features` a client must support before it
    /// may install this package. Default empty → no compatibility requirements, so
    /// every existing manifest (the spine demo, the lifecycle fixtures) installs
    /// unchanged. The capability-negotiation gate
    /// (`forge/spec/required-features.md`) reads `required_features` and REFUSES the
    /// install when the client does not support every requirement.
    #[serde(default)]
    pub compatibility: Compatibility,
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
        self.capabilities.validate()?;
        self.compatibility.validate()?;
        Ok(())
    }
}

/// Marketplace compatibility declaration (prd-merged/08 MP-4 / MP-8).
///
/// A package declares the host floor it needs to run: the minimum app version
/// (`min_app_version`) and the set of capability/runtime FEATURES the client must
/// support (`required_features`). The capability-negotiation gate
/// (`forge/spec/required-features.md`) refuses an install when the installing
/// client does not support **every** declared feature at the declared minimum
/// version — the MP-8 "refuse or limited-mode gracefully" contract.
///
/// Both fields default empty so a manifest that declares no compatibility floor
/// (every existing spine/lifecycle manifest) installs unchanged. The whole object
/// is preserved verbatim across re-encodings (the CRDT manifest document), so a
/// future client that adds a feature to its registry can install a package an
/// older client refused without the package changing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Compatibility {
    /// The minimum app version this package runs against (prd-merged/08 MP-4).
    /// `None` (the default) imposes no floor. Compared with the same dotted-numeric
    /// ordering as a feature's `min_version` (see [`version_at_least`]); the
    /// capability-negotiation gate can model it as a synthetic `app` feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
    /// The capability/runtime features this package requires the client to support
    /// (prd-merged/08 MP-8). Empty (the default) → no required features → the
    /// package installs on any client. Each entry names a feature id + the minimum
    /// version of that feature the package needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_features: Vec<FeatureRequirement>,
}

impl Compatibility {
    /// Validate the declared compatibility floor (prd-merged/01 CR-A4): every
    /// required feature must name a non-blank id and a well-formed dotted-numeric
    /// `min_version`, and `min_app_version` (when present) must be well-formed. A
    /// malformed requirement is a structural manifest error, not a silent pass that
    /// the negotiation gate would then have to interpret.
    pub fn validate(&self) -> crate::Result<()> {
        if let Some(v) = &self.min_app_version {
            if parse_version(v).is_none() {
                return Err(crate::CoreError::ValidationError(format!(
                    "compatibility.min_app_version must be dotted-numeric, got {v:?}"
                )));
            }
        }
        for req in &self.required_features {
            req.validate()?;
        }
        Ok(())
    }
}

/// One required capability/runtime feature (prd-merged/08 MP-8): the `feature_id`
/// the client must support and the `min_version` of it the package needs.
///
/// `feature_id` is a stable identifier (e.g. `ctx.db.query`, `ui.tabs`); it is
/// compared after [`normalize_feature_id`] so a package and a client that spell the
/// same feature with different case/surrounding whitespace still match. `min_version`
/// is a dotted-numeric version (`"1"`, `"1.2"`, `"1.2.0"`) compared with
/// [`version_at_least`] — a client supporting `1.3.0` satisfies a `1.2.0` floor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FeatureRequirement {
    /// The feature id the client must support. Normalized via
    /// [`normalize_feature_id`] before comparison.
    pub feature_id: String,
    /// The minimum dotted-numeric version of the feature the package needs.
    /// Default `"0"` → "any supported version" (presence alone satisfies it).
    #[serde(default = "default_min_version")]
    pub min_version: String,
}

fn default_min_version() -> String {
    "0".to_string()
}

impl FeatureRequirement {
    /// The feature id in canonical form for comparison (see [`normalize_feature_id`]).
    pub fn normalized_id(&self) -> String {
        normalize_feature_id(&self.feature_id)
    }

    /// Validate the requirement is well-formed: a non-blank (after normalization)
    /// feature id and a dotted-numeric `min_version`.
    pub fn validate(&self) -> crate::Result<()> {
        if self.normalized_id().is_empty() {
            return Err(crate::CoreError::ValidationError(
                "compatibility.required_features[].feature_id is empty".into(),
            ));
        }
        if parse_version(&self.min_version).is_none() {
            return Err(crate::CoreError::ValidationError(format!(
                "required feature {:?} min_version must be dotted-numeric, got {:?}",
                self.feature_id, self.min_version
            )));
        }
        Ok(())
    }
}

/// Canonicalize a feature id for comparison (prd-merged/08 MP-8, the
/// case/normalization vector): trim surrounding whitespace and lowercase with the
/// ASCII fold. Feature ids are ASCII identifiers (`ctx.db.query`, `ui.tabs`), so the
/// fold is deterministic and locale-independent — a package's `CTX.DB.Query` and a
/// client's `ctx.db.query` name the SAME feature. The same normalization is applied
/// to BOTH the package's required ids and the client registry's keys so the match
/// is symmetric.
pub fn normalize_feature_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

/// Parse a dotted-numeric version (`"1"`, `"1.2"`, `"1.2.0"`) into its numeric
/// components, or `None` when it is empty or carries a non-numeric component. Kept
/// minimal (no semver pre-release/build metadata) because MP-8 only needs a total
/// order over the simple feature versions the registry tracks; trailing zeros do
/// not matter (`1.2` == `1.2.0`) because [`version_at_least`] compares
/// component-wise and treats a missing component as `0`.
pub fn parse_version(v: &str) -> Option<Vec<u64>> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    v.split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()
        .filter(|parts| !parts.is_empty())
}

/// Whether `have` (the client's supported version) is at least `need` (the
/// package's required `min_version`), comparing dotted-numeric versions
/// component-wise with a missing component treated as `0` (so `1.2` ≥ `1.2.0` and
/// `1.3` ≥ `1.2.9`). A version that fails to parse compares as **not** at least the
/// requirement (fail-closed) — the negotiation gate never accepts an
/// uninterpretable version. Returns `true` only when both parse and `have ≥ need`.
pub fn version_at_least(have: &str, need: &str) -> bool {
    match (parse_version(have), parse_version(need)) {
        (Some(have), Some(need)) => {
            let len = have.len().max(need.len());
            for i in 0..len {
                let h = have.get(i).copied().unwrap_or(0);
                let n = need.get(i).copied().unwrap_or(0);
                if h != n {
                    return h > n;
                }
            }
            true
        }
        _ => false,
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
    /// Handle-scoped filesystem grants for `ctx.files` (prd-merged/01 CR-3,
    /// prd-merged/07 SC-8/SC-10/SC-12, `forge/spec/files.md`). Default empty →
    /// **no file access**: an applet that lists no `files` rules cannot read or
    /// write any file at all. Each rule names a user-granted *handle* (never a
    /// native absolute root — the handle resolves to a per-applet sandbox root
    /// via trusted policy at the host), a `path_glob` matched against the
    /// normalized relative path inside the handle, and per-action `max_bytes` /
    /// `content_types` constraints. The runtime's `ctx.files` host call evaluates
    /// a read/write against this grant before touching the host filesystem.
    #[serde(default)]
    pub files: FilesGrant,
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
            files: FilesGrant::default(),
        }
    }
}

impl Capabilities {
    pub fn validate(&self) -> crate::Result<()> {
        self.net.validate()
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

    pub fn validate(&self) -> crate::Result<()> {
        for rule in &self.0 {
            rule.validate()?;
        }
        Ok(())
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

impl NetRule {
    pub fn validate(&self) -> crate::Result<()> {
        let Some((scheme, _rest)) = self.url.split_once("://") else {
            return Err(crate::CoreError::ValidationError(format!(
                "net rule url must be absolute scheme://host/path, got {:?}",
                self.url
            )));
        };
        if !is_supported_net_scheme(scheme) {
            return Err(crate::CoreError::ValidationError(format!(
                "net rule url scheme {:?} is not supported; only http and https are allowed",
                scheme
            )));
        }
        Ok(())
    }
}

pub fn is_supported_net_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
}

/// The applet's handle-scoped filesystem grants (`ctx.files`).
///
/// prd-merged/01 CR-3, prd-merged/07 SC-8/SC-10/SC-12, `forge/spec/files.md`.
/// `read` and `write` are **separate arrays** so a review UI can show exactly
/// which file operations an applet requests. **Both empty = no file access**,
/// the default for every applet (an applet that lists no `files` rules cannot
/// touch the filesystem at all — distinct from a request that matches no rule of
/// a non-empty grant). Mirrors [`NetGrant`]'s read/write split + empty-default
/// semantics so manifests don't churn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FilesGrant {
    #[serde(default)]
    pub read: Vec<FileRule>,
    #[serde(default)]
    pub write: Vec<FileRule>,
}

impl FilesGrant {
    /// Whether the applet declared *any* file rule (read or write). An empty
    /// grant means the applet never requested the `files` capability at all —
    /// the runtime maps a file op against an empty action list to
    /// `CapabilityRequired`.
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty()
    }
}

/// One handle-scoped filesystem grant (prd-merged/07 SC-8 grammar,
/// `forge/spec/files.md`).
///
/// `handle` + `path_glob` are the action's resource: a stable logical `handle`
/// the trusted workspace/user policy maps to a per-applet sandbox root (the
/// manifest **never** names a native absolute root, SC-8/SC-12), plus a
/// `path_glob` matched against the normalized relative path inside that handle
/// (`*` matches within a path segment, `**` may cross segment boundaries). The
/// remaining fields are per-action constraints the runtime enforces *before*
/// returning a read response or committing a write payload.
///
/// Unknown constraint fields are tolerated for forward-compat (a future
/// constraint added to a fixture/manifest won't fail to parse); the runtime only
/// acts on the fields it knows. Field names mirror the `forge/fixtures/files/*`
/// vectors' `grant` shape so a fixture's grant deserializes straight into a
/// [`FilesGrant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileRule {
    /// The user-granted handle this rule applies to, e.g. `workspace_data`. The
    /// host's trusted policy maps it to a per-applet sandbox root; the manifest
    /// never names a native absolute filesystem path (SC-8/SC-12).
    pub handle: String,
    /// `path_glob` matched against the **normalized relative** path inside the
    /// handle, e.g. `data/**/*.json`. `*` matches within a single path segment;
    /// `**` may cross segment boundaries.
    pub path_glob: String,
    /// Max bytes a read response / write payload for this action may be (SC-5
    /// per-action budget). `None` = no rule-level cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Allowed `Content-Type` values for this action. Empty = unconstrained;
    /// non-empty = a read/write whose content-type is not in this set is denied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_types: Vec<String>,
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
            compatibility: Compatibility::default(),
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
            compatibility: Compatibility::default(),
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
            compatibility: Compatibility::default(),
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
        // An absent `compatibility` is the empty floor: no min app version, no
        // required features — so the package installs on any client (MP-8).
        assert!(m.compatibility.min_app_version.is_none());
        assert!(m.compatibility.required_features.is_empty());
        assert!(m.validate().is_ok());
    }

    // --- Compatibility / required_features (MP-8) ----------------------------

    #[test]
    fn compatibility_deserializes_from_manifest() {
        // A manifest writes `"compatibility": { min_app_version, required_features }`.
        let json = r#"{
            "entrypoint": "src/main.ts",
            "compatibility": {
                "min_app_version": "1.2.0",
                "required_features": [
                    { "feature_id": "ctx.db.query", "min_version": "1.0.0" },
                    { "feature_id": "ui.tabs" }
                ]
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.compatibility.min_app_version.as_deref(), Some("1.2.0"));
        assert_eq!(m.compatibility.required_features.len(), 2);
        assert_eq!(m.compatibility.required_features[0].feature_id, "ctx.db.query");
        assert_eq!(m.compatibility.required_features[0].min_version, "1.0.0");
        // An omitted `min_version` defaults to "0" (any supported version).
        assert_eq!(m.compatibility.required_features[1].min_version, "0");
        assert!(m.validate().is_ok());
    }

    #[test]
    fn feature_id_is_normalized_case_insensitively() {
        // The MP-8 case/normalization rule: surrounding whitespace + ASCII case
        // fold, applied symmetrically to package + client ids.
        assert_eq!(normalize_feature_id("  CTX.DB.Query "), "ctx.db.query");
        let req = FeatureRequirement {
            feature_id: "UI.Tabs".into(),
            min_version: "1.0".into(),
        };
        assert_eq!(req.normalized_id(), "ui.tabs");
    }

    #[test]
    fn version_at_least_is_a_dotted_numeric_total_order() {
        // Trailing-zero equivalence + component-wise compare; a higher client
        // version satisfies a lower floor (forward-compat).
        assert!(version_at_least("1.2", "1.2.0"));
        assert!(version_at_least("1.2.0", "1.2"));
        assert!(version_at_least("1.3.0", "1.2.9"));
        assert!(version_at_least("2.0.0", "1.9.9"));
        assert!(!version_at_least("1.1.0", "1.2.0"));
        // A malformed version on either side is fail-closed (never "at least").
        assert!(!version_at_least("not-a-version", "1.0.0"));
        assert!(!version_at_least("1.0.0", "also-bad"));
    }

    #[test]
    fn malformed_required_feature_is_a_validation_error() {
        // A blank feature id or a non-numeric min_version is a structural reject.
        let mut m: Manifest = serde_json::from_str(r#"{"entrypoint":"src/main.ts"}"#).unwrap();
        m.compatibility.required_features = vec![FeatureRequirement {
            feature_id: "   ".into(),
            min_version: "1.0.0".into(),
        }];
        assert_eq!(m.validate().unwrap_err().code(), "ValidationError");

        m.compatibility.required_features = vec![FeatureRequirement {
            feature_id: "ctx.db.query".into(),
            min_version: "one.two".into(),
        }];
        assert_eq!(m.validate().unwrap_err().code(), "ValidationError");

        // A malformed min_app_version is also rejected.
        let mut m2: Manifest = serde_json::from_str(r#"{"entrypoint":"src/main.ts"}"#).unwrap();
        m2.compatibility.min_app_version = Some("v1".into());
        assert_eq!(m2.validate().unwrap_err().code(), "ValidationError");
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

    #[test]
    fn net_rule_with_unsupported_scheme_is_validation_error() {
        let mut m: Manifest = serde_json::from_str(r#"{
            "entrypoint": "src/main.ts",
            "capabilities": {
                "net": [
                    { "method": "GET", "url": "ftp://api.example.com/files/*" }
                ]
            }
        }"#)
        .unwrap();
        let err = m.validate().unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("scheme"), "{err}");

        m.capabilities.net.0[0].url = "https://api.example.com/files/*".into();
        m.validate().unwrap();
    }

    // --- Files capability grant (CR-3 / spec/files.md) ----------------------

    #[test]
    fn files_grant_defaults_to_empty_no_file_access() {
        // An absent `capabilities` (and an absent `files`) grants zero file access.
        let m: Manifest = serde_json::from_str(r#"{"entrypoint":"src/main.ts"}"#).unwrap();
        assert!(m.capabilities.files.is_empty());
        assert!(m.capabilities.files.read.is_empty());
        assert!(m.capabilities.files.write.is_empty());
    }

    #[test]
    fn file_rule_deserializes_from_fixture_shaped_grant() {
        // The same JSON shape the T028 fixtures put under "files": a handle, a
        // normalized-relative path_glob, and the per-action constraint fields.
        let json = r#"{
            "handle": "workspace_data",
            "path_glob": "data/**/*.json",
            "max_bytes": 65536,
            "content_types": ["application/json"]
        }"#;
        let rule: FileRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.handle, "workspace_data");
        assert_eq!(rule.path_glob, "data/**/*.json");
        assert_eq!(rule.max_bytes, Some(65536));
        assert_eq!(rule.content_types, vec!["application/json".to_string()]);
    }

    #[test]
    fn files_grant_deserializes_as_read_write_arrays_in_capabilities() {
        // A manifest writes `"files": { "read": [...], "write": [...] }`. This is
        // the exact `grant_shape` in forge/fixtures/files/manifest.json.
        let json = r#"{
            "entrypoint": "src/main.ts",
            "capabilities": {
                "files": {
                    "read": [
                        { "handle": "workspace_data", "path_glob": "data/**/*.json",
                          "max_bytes": 65536, "content_types": ["application/json"] }
                    ],
                    "write": [
                        { "handle": "workspace_data", "path_glob": "drafts/*.txt",
                          "max_bytes": 65536, "content_types": ["text/plain"] }
                    ]
                }
            }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        let files = &m.capabilities.files;
        assert!(!files.is_empty());
        assert_eq!(files.read.len(), 1);
        assert_eq!(files.write.len(), 1);
        assert_eq!(files.read[0].handle, "workspace_data");
        assert_eq!(files.write[0].path_glob, "drafts/*.txt");
        // A `files` grant does not disturb the M0a ui-default.
        assert!(m.capabilities.ui);
    }

    #[test]
    fn file_rule_roundtrips_and_omits_empty_constraints() {
        let rule = FileRule {
            handle: "workspace_data".into(),
            path_glob: "data/*.json".into(),
            ..Default::default()
        };
        let s = serde_json::to_string(&rule).unwrap();
        let back: FileRule = serde_json::from_str(&s).unwrap();
        assert_eq!(rule, back);
        // Optional/empty constraints are omitted from the wire form (clean JSON).
        assert!(!s.contains("max_bytes"), "unset cap omitted: {s}");
        assert!(!s.contains("content_types"), "empty list omitted: {s}");
        assert!(s.contains("path_glob"), "required field present: {s}");
    }
}
