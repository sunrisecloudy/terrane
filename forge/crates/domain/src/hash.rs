//! Canonical content hashing for the forge core.
//!
//! [`code_hash`] is the **single source of truth** for the `code_hash` that
//! flows through the spine: the pipeline computes it over the transpiled JS and
//! the runtime records it on every [`RunRecord`](crate::run::RunRecord)
//! (prd-merged/01 CR-9, CR-14). Keeping the algorithm here — rather than each
//! crate re-implementing it — guarantees a real TS -> SWC -> run trace records
//! exactly the hash the pipeline promised (resolves review 010 P1, which found
//! the pipeline emitting `sha256:` while the runtime recorded a divergent
//! `fnv1a64:`).
//!
//! `sha2` is pure-Rust with no I/O, so this module keeps forge-domain
//! `wasm32-unknown-unknown`-clean.

use sha2::{Digest, Sha256};

/// Canonical content hash: `"sha256:" + lowercase-hex(sha256(code))`.
///
/// Deterministic and platform-stable: identical input bytes always yield the
/// same string, and any difference in input yields a different string. The
/// output is always the literal prefix `"sha256:"` followed by exactly 64
/// lowercase hexadecimal characters.
///
/// This is THE hash both forge-pipeline (over transpiled JS) and forge-runtime
/// (recorded on the run) must use, so provenance is preserved end-to-end.
pub fn code_hash(code: &str) -> String {
    let digest = Sha256::digest(code.as_bytes());
    // 7 chars for "sha256:" + two hex chars per digest byte (32 bytes -> 64).
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        // Lowercase hex, two chars per byte — deterministic and platform-stable.
        // `unwrap` is on a constant 0..=15 nibble, never a real-path failure.
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same input always hashes to the same string (stable across calls).
    #[test]
    fn code_hash_is_stable_for_same_input() {
        let code = "function main(ctx, input) { return { ok: true }; }";
        assert_eq!(code_hash(code), code_hash(code));
    }

    /// Different inputs hash to different strings (content-sensitive).
    #[test]
    fn code_hash_differs_for_different_input() {
        assert_ne!(code_hash("a"), code_hash("b"));
        // A single trailing byte must change the hash.
        assert_ne!(code_hash("main()"), code_hash("main() "));
    }

    /// Output carries the literal `"sha256:"` prefix.
    #[test]
    fn code_hash_has_sha256_prefix() {
        assert!(code_hash("").starts_with("sha256:"));
        assert!(code_hash("anything at all").starts_with("sha256:"));
    }

    /// The hex body is exactly 64 lowercase hex chars (256-bit digest).
    #[test]
    fn code_hash_body_is_64_lowercase_hex() {
        let h = code_hash("payload");
        let hex = h.strip_prefix("sha256:").expect("sha256: prefix");
        assert_eq!(hex.len(), 64, "sha256 hex must be 64 chars, got {}", hex.len());
        assert!(
            hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "hex body must be lowercase hex: {hex}"
        );
    }

    /// Known-answer vector: sha256 of the empty string is the well-known digest.
    /// This pins the algorithm so a future refactor cannot silently change it.
    #[test]
    fn code_hash_matches_known_empty_string_vector() {
        assert_eq!(
            code_hash(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// The function is byte-for-byte identical to what the pipeline previously
    /// computed inline (`"sha256:" + lowercase-hex`), so adopting it cannot
    /// change any already-recorded hash. Known-answer vector for "hello".
    #[test]
    fn code_hash_matches_known_hello_vector() {
        assert_eq!(
            code_hash("hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
