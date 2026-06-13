//! Data-driven verification against the T012 signing fixtures
//! (`forge/fixtures/signing/`). Every `valid` case must verify `Ok`; every
//! `invalid` case must be `Err` with the *exact* failure layer the fixture
//! declares (crypto vs package_hash vs policy). This is the proof that the Rust
//! preimage + hashing matches the bytes the fixtures actually signed.

use std::path::{Path, PathBuf};

use forge_signing::{
    package_preimage, verify_package, verify_signature, FailureLayer, Package, PublisherTrust,
    TrustOutcome,
};
use serde::Deserialize;
use serde_json::Value;

/// Absolute path to the fixtures directory, resolved from the crate manifest dir
/// so the test runs from any working directory.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/signing")
        .canonicalize()
        .expect("fixtures/signing directory must exist")
}

/// One fixture case. The publisher-trust block only appears on the policy cases.
#[derive(Debug, Deserialize)]
struct Case {
    case: String,
    package: Package,
    public_key_pem: String,
    signature: String,
    /// The exact UTF-8 bytes the fixture says were signed.
    signed_payload: String,
    /// Those same bytes as a UTF-8 hex string (cross-check).
    signed_payload_utf8_hex: String,
    expect: String,
    failure_layer: Option<FailureLayer>,
    #[serde(default)]
    publisher_trust: Option<FixturePublisherTrust>,
}

/// The fixtures' publisher-trust shape: an `unknown` status, or a `valid_until`
/// expiry on an otherwise-trusted publisher.
#[derive(Debug, Deserialize)]
struct FixturePublisherTrust {
    publisher: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    valid_until: Option<String>,
}

impl FixturePublisherTrust {
    /// Map the fixture trust block to the verifier's [`PublisherTrust`]:
    /// `status == "unknown"` → not trusted; a `valid_until` → trusted but with
    /// an expiry the verifier enforces against `signedAt`.
    fn to_trust(&self) -> PublisherTrust {
        let trusted = self.status.as_deref() != Some("unknown");
        PublisherTrust {
            publisher: self.publisher.clone(),
            trusted,
            valid_until: self.valid_until.clone(),
        }
    }
}

fn load_case(path: &Path) -> Case {
    let raw = std::fs::read_to_string(path).expect("read fixture");
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Discover every `valid_*.json` / `invalid_*.json` case file in the fixtures.
fn case_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("read fixtures dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (name.starts_with("valid_") || name.starts_with("invalid_")) && name.ends_with(".json")
        })
        .collect();
    files.sort();
    files
}

#[test]
fn fixtures_directory_is_present_and_nonempty() {
    let files = case_files();
    assert!(
        files.len() >= 14,
        "expected the full T012 vector set, found {} case files",
        files.len()
    );
}

#[test]
fn rust_preimage_matches_fixture_signed_payload() {
    // The preimage we reconstruct from the *live* package must equal the exact
    // bytes the fixture recorded as signed — for every case whose preimage-fields
    // are intact. This is the load-bearing proof that the Rust preimage
    // definition is byte-identical to what T012 signed.
    //
    // The one exception is `invalid_manifest_tampered`: it edits `appVersion`
    // (1.0.0 -> 1.0.1), which is a *plaintext preimage line*, so the live preimage
    // is intentionally different from the original `signed_payload`. We assert
    // that difference below in `manifest_tamper_diverges_from_signed_preimage`.
    for path in case_files() {
        let case = load_case(&path);
        if case.case == "invalid_manifest_tampered" {
            continue;
        }
        let preimage = package_preimage(&case.package)
            .unwrap_or_else(|e| panic!("{}: preimage build failed: {e}", case.case));

        assert_eq!(
            preimage,
            case.signed_payload.as_bytes(),
            "{}: reconstructed preimage != fixture signed_payload",
            case.case
        );

        let hex: String = preimage.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, case.signed_payload_utf8_hex,
            "{}: preimage hex != fixture signed_payload_utf8_hex",
            case.case
        );
    }
}

#[test]
fn manifest_tamper_diverges_from_signed_preimage() {
    // Editing a plaintext preimage field (appVersion) makes the live preimage
    // differ from the originally-signed bytes, so the signature cannot verify
    // against the live preimage — the manifestHash integrity check is what
    // attributes this to the package_hash layer (see the outcome test).
    let case = load_case(&fixtures_dir().join("invalid_manifest_tampered.json"));
    let preimage = package_preimage(&case.package).expect("preimage");
    assert_ne!(
        preimage,
        case.signed_payload.as_bytes(),
        "tampered appVersion must change the live preimage"
    );
    verify_signature(&preimage, &case.signature, &case.public_key_pem)
        .expect_err("signature must not verify over the tampered live preimage");
}

#[test]
fn every_case_resolves_to_the_declared_outcome() {
    let mut valid = 0usize;
    let mut crypto = 0usize;
    let mut package_hash = 0usize;
    let mut policy = 0usize;

    for path in case_files() {
        let case = load_case(&path);
        let trust = case.publisher_trust.as_ref().map(FixturePublisherTrust::to_trust);
        let outcome = verify_package(
            &case.package,
            &case.signature,
            &case.public_key_pem,
            trust.as_ref(),
        );

        match case.expect.as_str() {
            "valid" => {
                assert_eq!(
                    outcome,
                    TrustOutcome::Trusted,
                    "{}: expected Trusted, got {outcome:?}",
                    case.case
                );
                assert!(outcome.is_trusted());
                assert!(outcome.into_result().is_ok());
                valid += 1;
            }
            "invalid" => {
                let expected_layer = case
                    .failure_layer
                    .unwrap_or_else(|| panic!("{}: invalid case missing failure_layer", case.case));
                assert_eq!(
                    outcome.failure_layer(),
                    Some(expected_layer),
                    "{}: wrong failure layer; outcome = {outcome:?}",
                    case.case
                );
                // It must also surface as a typed CoreError::ValidationError.
                let err = outcome.into_result().expect_err("invalid case must Err");
                assert_eq!(err.code(), "ValidationError", "{}", case.case);
                match expected_layer {
                    FailureLayer::Crypto => crypto += 1,
                    FailureLayer::PackageHash => package_hash += 1,
                    FailureLayer::Policy => policy += 1,
                }
            }
            other => panic!("{}: unknown expect {other:?}", case.case),
        }
    }

    // Lock in the shape of the vector set so a dropped/renamed fixture is caught.
    assert_eq!(valid, 2, "valid case count");
    assert_eq!(crypto, 6, "crypto failure case count");
    assert_eq!(package_hash, 4, "package_hash failure case count");
    assert_eq!(policy, 2, "policy failure case count");
    assert_eq!(
        valid + crypto + package_hash + policy,
        14,
        "total T012 vector count"
    );
}

#[test]
fn valid_signature_verifies_at_the_crypto_layer_directly() {
    // The lowest-level check: the raw verify_signature over the reconstructed
    // preimage returns Ok for a valid case, independent of the package wrapper.
    let path = fixtures_dir().join("valid_signature.json");
    let case = load_case(&path);
    let preimage = package_preimage(&case.package).expect("preimage");
    verify_signature(&preimage, &case.signature, &case.public_key_pem)
        .expect("valid fixture must verify at the crypto layer");
}

#[test]
fn content_tamper_keeps_a_valid_signature_yet_fails_integrity() {
    // The file-content-tamper case proves the package_hash layer is *independent*
    // of crypto: only the live file content changed, so the recorded hashes (and
    // thus the preimage) are unchanged and the signature still verifies — yet
    // verify_package rejects at package_hash because the live content no longer
    // hashes to the recorded contentHash.
    let path = fixtures_dir().join("invalid_file_content_hash_mismatch.json");
    let case = load_case(&path);
    let preimage = package_preimage(&case.package).expect("preimage");
    // Crypto over the (unchanged) recorded-hash preimage is still valid...
    verify_signature(&preimage, &case.signature, &case.public_key_pem)
        .expect("recorded-hash preimage still verifies");
    // ...but the full package check catches the tamper at the integrity layer.
    let outcome = verify_package(&case.package, &case.signature, &case.public_key_pem, None);
    assert_eq!(outcome.failure_layer(), Some(FailureLayer::PackageHash));
}

#[test]
fn unsigned_manifest_helpers_round_trip_canonical_json() {
    // canonical_json of a fixture manifest is stable key-sorted with no spaces.
    let case = load_case(&fixtures_dir().join("valid_signature.json"));
    let manifest: &Value = &case.package.manifest;
    let json = forge_signing::canonical_json(manifest).expect("canonical");
    // Sorted keys: "appId" precedes "appVersion" precedes "capabilities" ...
    let app_id = json.find("\"appId\"").expect("appId present");
    let capabilities = json.find("\"capabilities\"").expect("capabilities present");
    assert!(app_id < capabilities, "keys must be sorted");
    assert!(!json.contains(": "), "canonical JSON has no insignificant spaces");
}
