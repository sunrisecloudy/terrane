//! Trust evaluation: turn a package + signature + key + (optional) publisher
//! trust into a [`TrustOutcome`], distinguishing the three failure layers.
//!
//! The split (SC-15 / MP-4) lets the platform surface *why* a package is not
//! trusted, which drives very different UX:
//!
//! - **crypto** — the bytes are not authentic; never install.
//! - **package_hash** — the bytes were authentic when signed but the package on
//!   disk no longer matches; it was tampered after signing — never install.
//! - **policy** — the bytes are authentic and intact, but *this* installer does
//!   not trust the publisher (unknown / expired). The crypto is fine; the
//!   marketplace policy says no. An operator could choose to trust the
//!   publisher and re-evaluate.

use crate::preimage::{
    content_hash, manifest_hash, package_preimage, permissions_hash, policy_hash,
};
use crate::verify::verify_signature;
use crate::Package;
use forge_domain::CoreError;
use serde::{Deserialize, Serialize};

/// Which layer rejected a package. Surfaced so the platform can report the
/// right trust result (SC-15 "policy vs crypto" split) and so tooling can assert
/// the *reason* a fixture is invalid, not merely that it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureLayer {
    /// The Ed25519 signature itself did not verify (wrong key, signature over
    /// different bytes, truncated/garbage/wrong-algorithm-label).
    Crypto,
    /// The signature verifies, but a file/manifest/permissions/policy region was
    /// changed after signing, so the package no longer matches the signed
    /// hashes (the integrity layer).
    PackageHash,
    /// Crypto + integrity are fine, but the publisher is not trusted by this
    /// installer (unknown / expired trust). A marketplace-policy decision.
    Policy,
}

impl FailureLayer {
    /// Stable machine token, matching the T012 fixtures' `failure_layer` strings.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureLayer::Crypto => "crypto",
            FailureLayer::PackageHash => "package_hash",
            FailureLayer::Policy => "policy",
        }
    }
}

/// A rejection: which layer caught it, plus a human-readable reason. Convertible
/// into the crate's [`CoreError::ValidationError`] for the FFI boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustError {
    pub layer: FailureLayer,
    pub reason: String,
}

impl TrustError {
    pub(crate) fn new(layer: FailureLayer, reason: impl Into<String>) -> Self {
        TrustError {
            layer,
            reason: reason.into(),
        }
    }
}

impl From<TrustError> for CoreError {
    fn from(e: TrustError) -> Self {
        // The reason already names the layer for logs/telemetry; keep it typed.
        CoreError::ValidationError(format!("{}: {}", e.layer.as_str(), e.reason))
    }
}

/// The result of evaluating a package's signature + trust. `Trusted` means the
/// crypto verified, the package matches the signed hashes, and (if a publisher
/// trust was supplied) the publisher is trusted. Any other variant carries the
/// failure layer + reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustOutcome {
    /// Verified + intact + (if checked) trusted publisher.
    Trusted,
    /// Rejected; see the [`TrustError`] for the layer + reason.
    Rejected(TrustError),
}

impl TrustOutcome {
    /// `true` iff the package is trusted.
    pub fn is_trusted(&self) -> bool {
        matches!(self, TrustOutcome::Trusted)
    }

    /// The failure layer, if this outcome is a rejection.
    pub fn failure_layer(&self) -> Option<FailureLayer> {
        match self {
            TrustOutcome::Trusted => None,
            TrustOutcome::Rejected(e) => Some(e.layer),
        }
    }

    /// Convert to a `Result`: `Ok(())` when trusted, else the typed error.
    pub fn into_result(self) -> Result<(), CoreError> {
        match self {
            TrustOutcome::Trusted => Ok(()),
            TrustOutcome::Rejected(e) => Err(e.into()),
        }
    }
}

/// What *this* installer knows about a publisher (the marketplace-policy input).
/// Supplying `None` to [`verify_package`] skips the policy layer entirely — the
/// M0a default of "verify crypto + integrity, do not yet enforce a trusted
/// publisher set". Supplying `Some(..)` enforces the publisher trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherTrust {
    /// The publisher id this record is about; must equal `manifest.publisher`.
    pub publisher: String,
    /// Whether the installer trusts this publisher at all (the unknown case).
    #[serde(default)]
    pub trusted: bool,
    /// RFC3339 instant after which the trust is expired, if any. The package is
    /// rejected when `manifest.signedAt` is at/after this instant.
    #[serde(default)]
    pub valid_until: Option<String>,
}

/// Recompute every component hash from the live package and confirm it matches
/// the recorded hash. This is the integrity (`package_hash`) layer: it catches a
/// file/manifest/permissions/policy region edited after signing, *even when the
/// signature still verifies* against the unchanged recorded-hash preimage.
fn check_integrity(pkg: &Package) -> Result<(), TrustError> {
    let pkg_hash = |reason: &str| TrustError::new(FailureLayer::PackageHash, reason);
    let from_core = |e: CoreError| TrustError::new(FailureLayer::PackageHash, e.to_string());

    if content_hash(&pkg.files) != pkg.hashes.content_hash {
        return Err(pkg_hash(
            "file content no longer matches the signed contentHash",
        ));
    }
    if manifest_hash(&pkg.manifest).map_err(from_core)? != pkg.hashes.manifest_hash {
        return Err(pkg_hash(
            "manifest changed after signing, so manifestHash differs",
        ));
    }
    if permissions_hash(&pkg.manifest).map_err(from_core)? != pkg.hashes.permissions_hash {
        return Err(pkg_hash(
            "permissions changed after signing, so permissionsHash differs",
        ));
    }
    if policy_hash(&pkg.manifest).map_err(from_core)? != pkg.hashes.policy_hash {
        return Err(pkg_hash(
            "policy (resourceBudget/networkPolicy/capabilities) changed after signing, so policyHash differs",
        ));
    }
    Ok(())
}

/// Evaluate the marketplace-policy layer: the publisher must match the manifest,
/// be trusted, and not be expired as of `signed_at`.
fn check_policy(publisher: &str, signed_at: &str, trust: &PublisherTrust) -> Result<(), TrustError> {
    if trust.publisher != publisher {
        return Err(TrustError::new(
            FailureLayer::Policy,
            format!(
                "publisher trust record is for {:?}, package is from {:?}",
                trust.publisher, publisher
            ),
        ));
    }
    if !trust.trusted {
        return Err(TrustError::new(
            FailureLayer::Policy,
            "publisher is not in the trusted publisher set",
        ));
    }
    if let Some(valid_until) = &trust.valid_until {
        // The fixtures use fixed-width RFC3339 Zulu timestamps, so a byte-wise
        // compare matches a chronological compare. Reject when the signature was
        // made at/after the trust expiry.
        if signed_at >= valid_until.as_str() {
            return Err(TrustError::new(
                FailureLayer::Policy,
                format!("publisher trust expired at {valid_until} before signedAt {signed_at}"),
            ));
        }
    }
    Ok(())
}

/// Read a required manifest string, attributing a missing field to the
/// `package_hash` layer (a structurally broken package).
fn manifest_field(pkg: &Package, key: &str) -> Result<String, TrustError> {
    pkg.manifest
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            TrustError::new(
                FailureLayer::PackageHash,
                format!("manifest.{key} is missing or not a string"),
            )
        })
}

/// Verify a package end to end and return a [`TrustOutcome`].
///
/// Evaluation order (so the *most fundamental* failure is reported first):
///
/// 1. **integrity** (`package_hash`) — recompute every component hash from the
///    live package; a mismatch means the package was tampered after signing.
/// 2. **crypto** — build the canonical preimage and verify the Ed25519
///    signature with `public_key`.
/// 3. **policy** — if `publisher_trust` is `Some`, the publisher must match the
///    manifest, be trusted, and not be expired as of `signedAt`.
///
/// Passing `publisher_trust = None` performs crypto + integrity only — the M0a
/// "verify when a signature is present, surface the result" default that does
/// not yet enforce a trusted-publisher set.
pub fn verify_package(
    pkg: &Package,
    signature: &str,
    public_key: &str,
    publisher_trust: Option<&PublisherTrust>,
) -> TrustOutcome {
    // 1) Integrity — does the package still match its signed hashes?
    if let Err(e) = check_integrity(pkg) {
        return TrustOutcome::Rejected(e);
    }

    // 2) Crypto — is the signature authentic over the canonical preimage?
    let preimage = match package_preimage(pkg) {
        Ok(p) => p,
        Err(e) => {
            return TrustOutcome::Rejected(TrustError::new(
                FailureLayer::PackageHash,
                e.to_string(),
            ))
        }
    };
    if let Err(e) = verify_signature(&preimage, signature, public_key) {
        return TrustOutcome::Rejected(TrustError::new(FailureLayer::Crypto, e.to_string()));
    }

    // 3) Policy — does this installer trust the publisher? (Optional in M0a.)
    if let Some(trust) = publisher_trust {
        let publisher = match manifest_field(pkg, "publisher") {
            Ok(p) => p,
            Err(e) => return TrustOutcome::Rejected(e),
        };
        let signed_at = match manifest_field(pkg, "signedAt") {
            Ok(s) => s,
            Err(e) => return TrustOutcome::Rejected(e),
        };
        if let Err(e) = check_policy(&publisher, &signed_at, trust) {
            return TrustOutcome::Rejected(e);
        }
    }

    TrustOutcome::Trusted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failure_layer_tokens_match_fixture_strings() {
        assert_eq!(FailureLayer::Crypto.as_str(), "crypto");
        assert_eq!(FailureLayer::PackageHash.as_str(), "package_hash");
        assert_eq!(FailureLayer::Policy.as_str(), "policy");
    }

    #[test]
    fn trust_error_converts_to_typed_validation_error_naming_the_layer() {
        let err: CoreError = TrustError::new(FailureLayer::Policy, "nope").into();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("policy"));
    }

    #[test]
    fn outcome_into_result_round_trips() {
        assert!(TrustOutcome::Trusted.is_trusted());
        assert!(TrustOutcome::Trusted.into_result().is_ok());
        let rejected = TrustOutcome::Rejected(TrustError::new(FailureLayer::Crypto, "bad"));
        assert_eq!(rejected.failure_layer(), Some(FailureLayer::Crypto));
        assert!(rejected.into_result().is_err());
    }

    #[test]
    fn policy_rejects_untrusted_publisher() {
        let trust = PublisherTrust {
            publisher: "p".into(),
            trusted: false,
            valid_until: None,
        };
        let err = check_policy("p", "2026-06-13T00:00:00Z", &trust).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);
        assert!(err.reason.contains("trusted publisher set"));
    }

    #[test]
    fn policy_rejects_publisher_mismatch() {
        let trust = PublisherTrust {
            publisher: "other".into(),
            trusted: true,
            valid_until: None,
        };
        let err = check_policy("p", "2026-06-13T00:00:00Z", &trust).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);
    }

    #[test]
    fn policy_enforces_expiry_against_signed_at() {
        let trust = PublisherTrust {
            publisher: "p".into(),
            trusted: true,
            valid_until: Some("2025-01-01T00:00:00Z".into()),
        };
        // signedAt after the expiry -> rejected
        let err = check_policy("p", "2026-06-13T00:00:00Z", &trust).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);
        // signedAt before the expiry -> ok
        assert!(check_policy("p", "2024-06-13T00:00:00Z", &trust).is_ok());
    }

    #[test]
    fn missing_manifest_publisher_is_a_package_hash_error() {
        let pkg = Package {
            manifest: json!({}),
            files: vec![],
            hashes: crate::PackageHashes {
                manifest_hash: String::new(),
                content_hash: String::new(),
                permissions_hash: String::new(),
                policy_hash: String::new(),
            },
        };
        let err = manifest_field(&pkg, "publisher").unwrap_err();
        assert_eq!(err.layer, FailureLayer::PackageHash);
    }
}
