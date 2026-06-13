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
    content_hash, file_digest, manifest_hash, package_preimage, permissions_hash, policy_hash,
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

    // Every recorded per-file digest must actually hash its content (review
    // 079 #1). MP-4 carries `files[{path, hash}]`; a package whose bytes are
    // intact but whose `files[].sha256` metadata was tampered to a bogus value
    // must not verify as trusted — downstream install/audit code reads that
    // metadata, so a trusted package must not carry a lying digest.
    for f in &pkg.files {
        if file_digest(&f.content) != f.sha256 {
            return Err(pkg_hash(
                "a file's recorded sha256 does not match its content digest",
            ));
        }
    }
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
        // Compare both timestamps as instants normalized to UTC (review 079 #3):
        // RFC3339 permits numeric offsets, so a raw lexicographic compare would
        // mis-order `…23:30:00Z` vs `…00:00:00+01:00`. Reject when the signature
        // was made at/after the trust expiry.
        let signed = parse_rfc3339(signed_at).map_err(|e| {
            TrustError::new(
                FailureLayer::Policy,
                format!("manifest.signedAt is not a valid RFC3339 instant: {e}"),
            )
        })?;
        let expiry = parse_rfc3339(valid_until).map_err(|e| {
            TrustError::new(
                FailureLayer::Policy,
                format!("publisher trust valid_until is not a valid RFC3339 instant: {e}"),
            )
        })?;
        if signed >= expiry {
            return Err(TrustError::new(
                FailureLayer::Policy,
                format!("publisher trust expired at {valid_until} before signedAt {signed_at}"),
            ));
        }
    }
    Ok(())
}

/// A normalized instant: UTC seconds since a fixed (year-0 proleptic) epoch plus
/// nanoseconds. Only used for ordering, so the epoch is arbitrary as long as it
/// is consistent. `Ord` derives lexicographically over `(seconds, nanos)`, which
/// is the correct chronological order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Instant {
    seconds: i64,
    nanos: u32,
}

/// Parse an RFC3339 date-time into a UTC-normalized [`Instant`], applying any
/// numeric offset (`Z`, `+hh:mm`, `-hh:mm`). Pure integer arithmetic — no chrono,
/// so the crate stays `wasm32-unknown-unknown`-clean. Rejects malformed input
/// with a typed reason rather than panicking.
///
/// Accepts `YYYY-MM-DDThh:mm:ss[.fraction](Z|±hh:mm)`. The `T` separator may be
/// upper or lower case (RFC3339 §5.6). Leap seconds (`ss == 60`) are accepted and
/// clamped to `59` for ordering purposes.
fn parse_rfc3339(s: &str) -> Result<Instant, String> {
    let bytes = s.as_bytes();
    // Minimum: `YYYY-MM-DDThh:mm:ssZ` = 20 chars.
    if bytes.len() < 20 {
        return Err(format!("{s:?} is too short"));
    }
    let num = |range: std::ops::Range<usize>| -> Result<i64, String> {
        s.get(range.clone())
            .and_then(|t| t.parse::<i64>().ok())
            .ok_or_else(|| format!("{s:?} has a non-numeric field at {range:?}"))
    };
    let sep = |idx: usize, ch: u8| -> Result<(), String> {
        if bytes.get(idx) == Some(&ch) {
            Ok(())
        } else {
            Err(format!("{s:?} is missing {:?} at position {idx}", ch as char))
        }
    };

    sep(4, b'-')?;
    sep(7, b'-')?;
    if !matches!(bytes.get(10), Some(b'T') | Some(b't')) {
        return Err(format!("{s:?} is missing the date/time 'T' separator"));
    }
    sep(13, b':')?;
    sep(16, b':')?;

    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let mut second = num(17..19)?;

    if !(1..=12).contains(&month) {
        return Err(format!("{s:?} month {month} out of range"));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("{s:?} day {day} out of range"));
    }
    if hour > 23 {
        return Err(format!("{s:?} hour {hour} out of range"));
    }
    if minute > 59 {
        return Err(format!("{s:?} minute {minute} out of range"));
    }
    if second == 60 {
        second = 59; // leap second: clamp for ordering
    } else if second > 59 {
        return Err(format!("{s:?} second {second} out of range"));
    }

    // Optional fractional seconds after `ss`, then the offset.
    let mut idx = 19;
    let mut nanos: u32 = 0;
    if bytes.get(idx) == Some(&b'.') {
        idx += 1;
        let start = idx;
        while bytes.get(idx).is_some_and(|b| b.is_ascii_digit()) {
            idx += 1;
        }
        if idx == start {
            return Err(format!("{s:?} has a '.' with no fractional digits"));
        }
        // Take up to 9 digits (nanosecond precision); pad to 9.
        let frac = &s[start..idx];
        let take = frac.len().min(9);
        let mut scaled: u64 = frac[..take].parse().map_err(|_| format!("{s:?} bad fraction"))?;
        for _ in take..9 {
            scaled *= 10;
        }
        nanos = scaled as u32;
    }

    // Offset: `Z`/`z` (UTC) or `±hh:mm`.
    let offset_minutes: i64 = match bytes.get(idx) {
        Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(idx + 1..idx + 3)?;
            sep(idx + 3, b':')?;
            let om = num(idx + 4..idx + 6)?;
            if oh > 23 || om > 59 {
                return Err(format!("{s:?} offset out of range"));
            }
            let mag = oh * 60 + om;
            if *sign == b'-' {
                -mag
            } else {
                mag
            }
        }
        _ => return Err(format!("{s:?} is missing a 'Z' or numeric UTC offset")),
    };

    // Days since a proleptic year-0 epoch (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let mut seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    // Convert local-offset time to UTC by subtracting the offset.
    seconds -= offset_minutes * 60;

    Ok(Instant { seconds, nanos })
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
    fn expiry_compares_instants_across_offsets_not_raw_strings() {
        // review 079 #3: `2026-06-12T23:30:00Z` is chronologically BEFORE
        // `2026-06-13T00:00:00+01:00` (which is 2026-06-12T23:00:00Z), so the
        // signature is AFTER the expiry and must be rejected — even though the
        // raw strings compare the other way (`2026-06-12…` < `2026-06-13…`).
        let trust = PublisherTrust {
            publisher: "p".into(),
            trusted: true,
            valid_until: Some("2026-06-13T00:00:00+01:00".into()),
        };
        let err = check_policy("p", "2026-06-12T23:30:00Z", &trust).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);
        assert!(err.reason.contains("expired"));

        // And a signature comfortably before that same expiry is accepted.
        assert!(check_policy("p", "2026-06-12T22:00:00Z", &trust).is_ok());
    }

    #[test]
    fn malformed_timestamps_are_rejected_at_the_policy_layer() {
        let trust = PublisherTrust {
            publisher: "p".into(),
            trusted: true,
            valid_until: Some("2026-13-99T99:99:99Z".into()),
        };
        let err = check_policy("p", "2026-06-13T00:00:00Z", &trust).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);

        let trust_ok = PublisherTrust {
            publisher: "p".into(),
            trusted: true,
            valid_until: Some("2030-01-01T00:00:00Z".into()),
        };
        let err = check_policy("p", "not-a-timestamp", &trust_ok).unwrap_err();
        assert_eq!(err.layer, FailureLayer::Policy);
        assert!(err.reason.contains("signedAt"));
    }

    #[test]
    fn rfc3339_parser_normalizes_offsets_and_fractions() {
        // Same instant, three spellings, must compare equal.
        let z = parse_rfc3339("2026-06-13T00:00:00Z").unwrap();
        let plus = parse_rfc3339("2026-06-13T01:00:00+01:00").unwrap();
        let minus = parse_rfc3339("2026-06-12T23:00:00-01:00").unwrap();
        assert_eq!(z, plus);
        assert_eq!(z, minus);

        // Fractional seconds order within the same second.
        let a = parse_rfc3339("2026-06-13T00:00:00.100Z").unwrap();
        let b = parse_rfc3339("2026-06-13T00:00:00.200Z").unwrap();
        assert!(a < b);
        assert!(z < a);

        // Lowercase `t`/`z` separators are accepted (RFC3339 §5.6).
        assert_eq!(parse_rfc3339("2026-06-13t00:00:00z").unwrap(), z);

        // Leap second clamps rather than erroring.
        assert!(parse_rfc3339("2026-06-30T23:59:60Z").is_ok());

        // Garbage is rejected.
        assert!(parse_rfc3339("2026/06/13 00:00:00").is_err());
        assert!(parse_rfc3339("").is_err());
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
