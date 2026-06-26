//! Byte-for-byte interoperability vectors for control tokens and shell signing
//! (E13). These lock the Rust implementation to the macOS/Linux/native-shell
//! algorithms captured before migration.

use base64::Engine;
use forge_signing::{
    encode_control_token, encode_control_token_from_entropy_b64, package_preimage,
    signature_payload_from_parts, verify_shell_signature, verify_signature, Package,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/signing")
        .canonicalize()
        .expect("fixtures/signing directory must exist")
}

#[derive(Debug, Deserialize)]
struct ValidSignatureCase {
    package: Package,
    public_key_pem: String,
    signature: String,
    signed_payload: String,
}

fn load_valid_signature() -> ValidSignatureCase {
    let path = fixtures_dir().join("valid_signature.json");
    let raw = std::fs::read_to_string(&path).expect("read valid_signature fixture");
    serde_json::from_str(&raw).expect("parse valid_signature fixture")
}

#[test]
fn control_token_vectors_match_macos_linux_encoding() {
    // macOS: SecRandomCopyBytes → standard base64 → URL-safe transform.
    let all_zero = [0u8; 32];
    assert_eq!(
        encode_control_token(&all_zero),
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    );

    let sequential: [u8; 32] = core::array::from_fn(|i| i as u8);
    let standard = base64::engine::general_purpose::STANDARD.encode(sequential);
    let expected = standard
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "");
    assert_eq!(encode_control_token(&sequential), expected);
    assert_eq!(
        encode_control_token_from_entropy_b64(&standard).expect("entropy b64"),
        expected
    );

    // Linux g_base64_encode of 0x00..0x1f with the same +/=_ transform.
    let linux_fixture_entropy = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    assert_eq!(
        encode_control_token_from_entropy_b64(linux_fixture_entropy).expect("linux entropy"),
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
    );
}

#[test]
fn signature_payload_matches_t012_signed_payload() {
    let case = load_valid_signature();
    let manifest = &case.package.manifest;
    let payload = signature_payload_from_parts(
        manifest["appId"].as_str().expect("appId"),
        manifest["appVersion"].as_str().expect("appVersion"),
        manifest["dataVersion"]
            .as_str()
            .expect("dataVersion")
            .parse()
            .expect("dataVersion int"),
        manifest["runtimeVersion"].as_str().expect("runtimeVersion"),
        manifest["trustLevel"].as_str().expect("trustLevel"),
        manifest["keyId"].as_str().expect("keyId"),
        &case.package.hashes.manifest_hash,
        &case.package.hashes.content_hash,
        &case.package.hashes.permissions_hash,
        &case.package.hashes.policy_hash,
        manifest["signedAt"].as_str().expect("signedAt"),
    );
    assert_eq!(payload, case.signed_payload);
    let preimage = package_preimage(&case.package).expect("package preimage");
    assert_eq!(payload.as_bytes(), preimage.as_slice());
}

#[test]
fn shell_signature_verifies_with_raw_base64_forms() {
    let case = load_valid_signature();
    let preimage = package_preimage(&case.package).expect("preimage");

    // T012 labelled form still works through verify_signature.
    verify_signature(&preimage, &case.signature, &case.public_key_pem)
        .expect("labelled signature verifies");

    // macOS shell stores raw base64 signature bytes and raw public key bytes.
    let (_, sig_body) = case.signature.split_once(':').expect("ed25519 label");
    let pem_body: String = case
        .public_key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let spki = base64::engine::general_purpose::STANDARD
        .decode(pem_body.trim().as_bytes())
        .expect("pem decode");
    let raw_pk = &spki[12..44];
    let raw_pk_b64 = base64::engine::general_purpose::STANDARD.encode(raw_pk);

    verify_shell_signature(&preimage, sig_body, &raw_pk_b64)
        .expect("raw shell signature verifies");
}