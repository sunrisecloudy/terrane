//! Build the `terrane/sig/v1` signed payload from explicit fields (E13).
//!
//! Package signing in native shells constructs this string, signs it with a
//! platform-held Ed25519 private key, and stores the raw base64 signature on the
//! package record. The line order matches macOS `signaturePayload` and the T012
//! fixtures consumed by [`crate::package_preimage`].

use crate::SIG_DOMAIN_TAG;

/// The preimage fields for a `terrane/sig/v1` package signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignaturePayloadFields<'a> {
    pub app_id: &'a str,
    pub app_version: &'a str,
    pub data_version: &'a str,
    pub runtime_version: &'a str,
    pub trust_level: &'a str,
    pub key_id: &'a str,
    pub manifest_hash: &'a str,
    pub content_hash: &'a str,
    pub permissions_hash: &'a str,
    pub policy_hash: &'a str,
    pub signed_at: &'a str,
}

/// Build the exact UTF-8 string that native shells sign (no trailing newline).
pub fn signature_payload(fields: &SignaturePayloadFields<'_>) -> String {
    [
        SIG_DOMAIN_TAG,
        fields.app_id,
        fields.app_version,
        fields.data_version,
        fields.runtime_version,
        fields.trust_level,
        fields.key_id,
        fields.manifest_hash,
        fields.content_hash,
        fields.permissions_hash,
        fields.policy_hash,
        fields.signed_at,
    ]
    .join("\n")
}

/// Convenience wrapper when `data_version` is an integer (macOS `String(dataVersion)`).
#[allow(clippy::too_many_arguments)]
pub fn signature_payload_from_parts(
    app_id: &str,
    app_version: &str,
    data_version: i64,
    runtime_version: &str,
    trust_level: &str,
    key_id: &str,
    manifest_hash: &str,
    content_hash: &str,
    permissions_hash: &str,
    policy_hash: &str,
    signed_at: &str,
) -> String {
    signature_payload(&SignaturePayloadFields {
        app_id,
        app_version,
        data_version: &data_version.to_string(),
        runtime_version,
        trust_level,
        key_id,
        manifest_hash,
        content_hash,
        permissions_hash,
        policy_hash,
        signed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_has_no_trailing_newline_and_starts_with_domain_tag() {
        let payload = signature_payload_from_parts(
            "app.notes",
            "1.0.0",
            1,
            "forge-m0a",
            "signed-test",
            "test-ed25519-2026-06",
            "sha256:aa",
            "sha256:bb",
            "sha256:cc",
            "sha256:dd",
            "2026-06-13T00:00:00Z",
        );
        assert!(payload.starts_with("terrane/sig/v1\n"));
        assert!(payload.ends_with("2026-06-13T00:00:00Z"));
        assert!(!payload.ends_with('\n'));
    }
}