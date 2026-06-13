//! The cryptographic layer: parse an Ed25519 public key + signature out of
//! their string encodings and verify a signature over a preimage.
//!
//! This module is *only* the crypto check — it knows nothing about packages or
//! trust policy. Every failure here is the `crypto` failure layer.

use crate::{validation_error, SigResult};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey, SIGNATURE_LENGTH};

/// The only algorithm label this verifier accepts on a signature or `ed25519:`
/// key string. A production verifier must reject any other label (e.g. the
/// `none-dev:` dev-mode label) rather than silently treating it as Ed25519.
pub const ALGORITHM_LABEL: &str = "ed25519";

/// Length of a raw (un-prefixed) Ed25519 public key, in bytes.
const PUBLIC_KEY_LENGTH: usize = ed25519_dalek::PUBLIC_KEY_LENGTH;

/// The 12-byte ASN.1/DER SubjectPublicKeyInfo prefix for an Ed25519 key
/// (`SEQUENCE { SEQUENCE { OID 1.3.101.112 } BIT STRING }`). An SPKI-encoded
/// Ed25519 public key is always this prefix followed by the 32 raw key bytes.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Strip the `ed25519:` prefix and base64-decode the body, rejecting any other
/// algorithm label. Used for both the signature and an `ed25519:`-form key.
fn decode_labelled(input: &str, what: &str) -> SigResult<Vec<u8>> {
    let (label, body) = input
        .split_once(':')
        .ok_or_else(|| validation_error(format!("{what} is missing an algorithm label")))?;
    if label != ALGORITHM_LABEL {
        return Err(validation_error(format!(
            "{what} uses unsupported algorithm label {label:?}; only {ALGORITHM_LABEL:?} is accepted"
        )));
    }
    BASE64
        .decode(body.as_bytes())
        .map_err(|e| validation_error(format!("{what} is not valid base64: {e}")))
}

/// Parse the 64-byte Ed25519 signature out of an `ed25519:<base64>` string.
fn parse_signature(signature: &str) -> SigResult<Signature> {
    let bytes = decode_labelled(signature, "signature")?;
    if bytes.len() != SIGNATURE_LENGTH {
        return Err(validation_error(format!(
            "signature is {} bytes, expected {SIGNATURE_LENGTH}",
            bytes.len()
        )));
    }
    // `try_into` cannot fail given the length check above; map the error rather
    // than unwrap so there is no panic on any path.
    let arr: [u8; SIGNATURE_LENGTH] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| validation_error("signature has an unexpected length"))?;
    Ok(Signature::from_bytes(&arr))
}

/// Extract the 32 raw public-key bytes from a `public_key` string. Two
/// encodings are accepted:
///
/// 1. `ed25519:<base64-of-32-raw-bytes>` — the spec's `ed25519:` key form;
/// 2. a PEM `-----BEGIN PUBLIC KEY-----` SPKI block — the form the T012
///    fixtures carry (`public_key_pem`).
fn parse_public_key(public_key: &str) -> SigResult<VerifyingKey> {
    let trimmed = public_key.trim();
    let raw: Vec<u8> = if trimmed.contains("BEGIN PUBLIC KEY") {
        let der = decode_pem(trimmed)?;
        // SPKI: fixed 12-byte Ed25519 prefix + 32 raw key bytes.
        if der.len() != ED25519_SPKI_PREFIX.len() + PUBLIC_KEY_LENGTH
            || der[..ED25519_SPKI_PREFIX.len()] != ED25519_SPKI_PREFIX
        {
            return Err(validation_error(
                "public key PEM is not a well-formed Ed25519 SubjectPublicKeyInfo",
            ));
        }
        der[ED25519_SPKI_PREFIX.len()..].to_vec()
    } else {
        decode_labelled(trimmed, "public key")?
    };

    if raw.len() != PUBLIC_KEY_LENGTH {
        return Err(validation_error(format!(
            "public key is {} bytes, expected {PUBLIC_KEY_LENGTH}",
            raw.len()
        )));
    }
    let arr: [u8; PUBLIC_KEY_LENGTH] = raw
        .as_slice()
        .try_into()
        .map_err(|_| validation_error("public key has an unexpected length"))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| validation_error(format!("public key is not a valid Ed25519 point: {e}")))
}

/// Base64-decode the body of a single PEM block (the lines between the
/// `-----BEGIN ...-----` / `-----END ...-----` markers).
fn decode_pem(pem: &str) -> SigResult<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    BASE64
        .decode(body.trim().as_bytes())
        .map_err(|e| validation_error(format!("public key PEM body is not valid base64: {e}")))
}

/// Verify an Ed25519 `signature` over `preimage` with `public_key`.
///
/// - `signature` is `ed25519:<base64-of-64-bytes>`.
/// - `public_key` is either `ed25519:<base64-of-32-bytes>` or a PEM SPKI block.
///
/// Returns `Ok(())` only if the signature is a valid Ed25519 signature over
/// exactly `preimage` for `public_key`. Every failure — bad label, bad base64,
/// wrong length, non-point key, or a signature that simply does not verify — is
/// a [`CoreError::ValidationError`](forge_domain::CoreError::ValidationError),
/// the `crypto` failure layer. Never panics.
pub fn verify_signature(preimage: &[u8], signature: &str, public_key: &str) -> SigResult<()> {
    let key = parse_public_key(public_key)?;
    let sig = parse_signature(signature)?;
    key.verify_strict(preimage, &sig)
        .map_err(|_| validation_error("Ed25519 signature does not verify over the signed preimage"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The test keypair from fixtures/signing/test-keypair.json: the raw public
    // key and a signature over the literal bytes `b"hello"` produced with the
    // matching seed (000102..1e1f). These let the crypto layer be exercised
    // without loading the package fixtures.
    const PK_PEM: &str =
        "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAA6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=\n-----END PUBLIC KEY-----\n";
    const PK_RAW_B64: &str = "ed25519:A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=";

    #[test]
    fn pem_and_raw_ed25519_key_forms_parse_to_the_same_key() {
        let from_pem = parse_public_key(PK_PEM).expect("pem parses");
        let from_raw = parse_public_key(PK_RAW_B64).expect("raw ed25519: parses");
        assert_eq!(from_pem.to_bytes(), from_raw.to_bytes());
    }

    #[test]
    fn signature_missing_label_is_rejected() {
        let err = parse_signature("AAAA").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn wrong_algorithm_label_on_signature_is_rejected() {
        let err = parse_signature("none-dev:AAAA").unwrap_err();
        assert!(err.to_string().contains("unsupported algorithm label"));
    }

    #[test]
    fn garbage_base64_signature_is_rejected() {
        let err = parse_signature("ed25519:not-base64!").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn truncated_signature_is_rejected_for_length() {
        // Valid base64 but far fewer than 64 bytes.
        let err = parse_signature("ed25519:AAAA").unwrap_err();
        assert!(err.to_string().contains("expected 64"));
    }

    #[test]
    fn malformed_pem_spki_is_rejected() {
        // A PEM body that base64-decodes to the wrong SPKI shape.
        let bad = "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n";
        let err = parse_public_key(bad).unwrap_err();
        assert!(err.to_string().contains("SubjectPublicKeyInfo"));
    }

    #[test]
    fn wrong_length_raw_key_is_rejected() {
        let err = parse_public_key("ed25519:AAAA").unwrap_err();
        assert!(err.to_string().contains("expected 32"));
    }
}
