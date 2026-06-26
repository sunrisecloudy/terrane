//! Control token + `terrane/sig/v1` signing seam (E13).
//!
//! Key custody stays in the shell: these commands format payloads and verify
//! signatures; the platform holds the Ed25519 private key (Keychain/CNG/etc.).

use forge_domain::{CoreError, Result};
use forge_signing::{
    encode_control_token_from_entropy_b64, signature_payload_from_parts, verify_shell_signature,
};
use serde_json::{json, Value};

fn validation_error(msg: impl Into<String>) -> CoreError {
    CoreError::ValidationError(msg.into())
}

fn required_str<'a>(payload: &'a Value, key: &str) -> Result<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| validation_error(format!("{key} is required")))
}

fn required_i64(payload: &Value, key: &str) -> Result<i64> {
    payload
        .get(key)
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .ok_or_else(|| validation_error(format!("{key} is required")))
}

/// `control.generate_token` — canonical URL-safe token from 32 entropy bytes.
///
/// Payload: `{ "entropy": "<standard base64 of 32 bytes>" }`
/// Response: `{ "token": "<url-safe unpadded base64>" }`
pub fn generate_token_from_payload(payload: &Value) -> Result<Value> {
    let entropy = required_str(payload, "entropy")?;
    let token = encode_control_token_from_entropy_b64(entropy)?;
    Ok(json!({ "token": token }))
}

/// `control.sign_payload` — build the `terrane/sig/v1` preimage string.
///
/// Payload carries the eleven preimage fields (see docs/17). Response:
/// `{ "payload": "<signed UTF-8 string>" }`.
pub fn sign_payload_from_payload(payload: &Value) -> Result<Value> {
    let signed = signature_payload_from_parts(
        required_str(payload, "appId")?,
        required_str(payload, "appVersion")?,
        required_i64(payload, "dataVersion")?,
        required_str(payload, "runtimeVersion")?,
        required_str(payload, "trustLevel")?,
        required_str(payload, "keyId")?,
        required_str(payload, "manifestHash")?,
        required_str(payload, "contentHash")?,
        required_str(payload, "permissionsHash")?,
        required_str(payload, "policyHash")?,
        required_str(payload, "signedAt")?,
    );
    Ok(json!({ "payload": signed }))
}

/// `control.verify_signature` — verify an Ed25519 signature over a payload.
///
/// Payload: `{ "payload", "signature", "publicKey" }` where `signature` and
/// `publicKey` may be raw shell base64 or the labelled T012 forms.
/// Response: `{ "ok": true }` on success.
pub fn verify_signature_from_payload(payload: &Value) -> Result<Value> {
    let preimage = required_str(payload, "payload")?;
    let signature = required_str(payload, "signature")?;
    let public_key = required_str(payload, "publicKey")?;
    verify_shell_signature(preimage.as_bytes(), signature, public_key)?;
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use forge_signing::{encode_control_token, verify_signature};
    use serde_json::json;
    use std::path::PathBuf;

    fn valid_signature_fixture() -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/signing/valid_signature.json");
        let raw = std::fs::read_to_string(path).expect("read fixture");
        serde_json::from_str(&raw).expect("parse fixture")
    }

    #[test]
    fn generate_token_command_matches_direct_encode() {
        let entropy = [0xABu8; 32];
        let entropy_b64 = base64::engine::general_purpose::STANDARD.encode(entropy);
        let result = generate_token_from_payload(&json!({ "entropy": entropy_b64 })).unwrap();
        assert_eq!(
            result["token"].as_str().unwrap(),
            encode_control_token(&entropy)
        );
    }

    #[test]
    fn sign_payload_command_matches_fixture_signed_payload() {
        let fixture = valid_signature_fixture();
        let manifest = &fixture["package"]["manifest"];
        let hashes = &fixture["package"]["hashes"];
        let result = sign_payload_from_payload(&json!({
            "appId": manifest["appId"],
            "appVersion": manifest["appVersion"],
            "dataVersion": manifest["dataVersion"],
            "runtimeVersion": manifest["runtimeVersion"],
            "trustLevel": manifest["trustLevel"],
            "keyId": manifest["keyId"],
            "manifestHash": hashes["manifestHash"],
            "contentHash": hashes["contentHash"],
            "permissionsHash": hashes["permissionsHash"],
            "policyHash": hashes["policyHash"],
            "signedAt": manifest["signedAt"],
        }))
        .unwrap();
        assert_eq!(
            result["payload"].as_str().unwrap(),
            fixture["signed_payload"].as_str().unwrap()
        );
    }

    #[test]
    fn verify_signature_command_accepts_labelled_fixture_form() {
        let fixture = valid_signature_fixture();
        let result = verify_signature_from_payload(&json!({
            "payload": fixture["signed_payload"],
            "signature": fixture["signature"],
            "publicKey": fixture["public_key_pem"],
        }))
        .unwrap();
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn verify_signature_command_accepts_raw_shell_base64() {
        let fixture = valid_signature_fixture();
        let signature = fixture["signature"].as_str().unwrap();
        let (_, sig_body) = signature.split_once(':').unwrap();
        let pem = fixture["public_key_pem"].as_str().unwrap();
        let pem_body: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<Vec<_>>()
            .join("");
        let spki = base64::engine::general_purpose::STANDARD
            .decode(pem_body.trim().as_bytes())
            .unwrap();
        let raw_pk_b64 =
            base64::engine::general_purpose::STANDARD.encode(&spki[12..44]);

        let result = verify_signature_from_payload(&json!({
            "payload": fixture["signed_payload"],
            "signature": sig_body,
            "publicKey": raw_pk_b64,
        }))
        .unwrap();
        assert_eq!(result["ok"], true);

        // Sanity: the labelled path still verifies at the crypto layer.
        verify_signature(
            fixture["signed_payload"].as_str().unwrap().as_bytes(),
            signature,
            pem,
        )
        .expect("labelled verify");
    }
}