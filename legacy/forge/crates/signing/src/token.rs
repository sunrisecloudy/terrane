//! Control-plane session token encoding (E13).
//!
//! Native shells draw 32 random bytes from the OS CSPRNG (Keychain-adjacent
//! custody stays per-platform) and pass them here to obtain the canonical
//! URL-safe, unpadded base64 token string. The algorithm matches macOS
//! `DevControlPlane.generateToken`, Linux `generate_control_token`, and
//! `tools/control-token.js` (`base64url`).

use crate::{validation_error, SigResult};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

/// Length of the entropy input for a control token, in bytes.
pub const CONTROL_TOKEN_ENTROPY_BYTES: usize = 32;

/// Encode exactly 32 entropy bytes into the canonical control token string.
///
/// Steps (byte-for-byte with the macOS shell):
/// 1. Standard base64-encode the 32 bytes (with `=` padding).
/// 2. Replace `+` → `-`, `/` → `_`.
/// 3. Strip all `=` padding characters.
pub fn encode_control_token(entropy: &[u8; CONTROL_TOKEN_ENTROPY_BYTES]) -> String {
    BASE64
        .encode(entropy)
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "")
}

/// Decode a standard-base64 entropy string from a shell and emit the token.
///
/// Shells typically send `Data(bytes).base64EncodedString()` (Swift) or the
/// equivalent standard base64 of the 32 raw bytes.
pub fn encode_control_token_from_entropy_b64(entropy_b64: &str) -> SigResult<String> {
    let bytes = BASE64
        .decode(entropy_b64.trim().as_bytes())
        .map_err(|e| validation_error(format!("entropy is not valid base64: {e}")))?;
    if bytes.len() != CONTROL_TOKEN_ENTROPY_BYTES {
        return Err(validation_error(format!(
            "entropy is {} bytes, expected {CONTROL_TOKEN_ENTROPY_BYTES}",
            bytes.len()
        )));
    }
    let arr: [u8; CONTROL_TOKEN_ENTROPY_BYTES] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| validation_error("entropy has an unexpected length"))?;
    Ok(encode_control_token(&arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_entropy_matches_macos_token_shape() {
        let entropy = [0u8; CONTROL_TOKEN_ENTROPY_BYTES];
        let token = encode_control_token(&entropy);
        assert_eq!(token.len(), 43);
        assert!(token.chars().all(|c| c == 'A'));
        assert!(!token.contains('='));
        assert!(!token.contains('+'));
        assert!(!token.contains('/'));
    }

    #[test]
    fn sequential_entropy_is_url_safe_without_padding() {
        let entropy: [u8; CONTROL_TOKEN_ENTROPY_BYTES] = core::array::from_fn(|i| i as u8);
        let token = encode_control_token(&entropy);
        assert!(!token.contains('='));
        assert!(!token.contains('+'));
        assert!(!token.contains('/'));
        // Cross-check the standard-base64 round-trip entry point.
        let standard = BASE64.encode(entropy);
        assert_eq!(
            token,
            standard
                .replace('+', "-")
                .replace('/', "_")
                .replace('=', "")
        );
    }

    #[test]
    fn entropy_b64_round_trip_matches_direct_encode() {
        let entropy: [u8; CONTROL_TOKEN_ENTROPY_BYTES] = [0x01; CONTROL_TOKEN_ENTROPY_BYTES];
        let b64 = BASE64.encode(entropy);
        assert_eq!(
            encode_control_token_from_entropy_b64(&b64).unwrap(),
            encode_control_token(&entropy)
        );
    }

    #[test]
    fn wrong_entropy_length_is_typed_error() {
        let err = encode_control_token_from_entropy_b64("AAAA").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }
}