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
//! [`is_canonical_code_hash`] is the shared predicate that gives that contract
//! *teeth*: it rejects a `code_hash` that is not the canonical `sha256:` form,
//! so a crate that fails to adopt [`code_hash`] (e.g. one still emitting
//! `fnv1a64:…`) can be caught instead of silently storing a divergent
//! provenance string. The predicate only bites where a caller invokes it:
//! [`RunRecord::validate_code_hash`] exposes it at the run-record boundary and
//! [`RunRecord::new`] makes it non-bypassable for callers that build a record
//! in one shot. Recording/replay/storage boundaries in other crates must adopt
//! one of those entry points for the check to fire there.
//!
//! [`RunRecord::validate_code_hash`]: crate::run::RunRecord::validate_code_hash
//! [`RunRecord::new`]: crate::run::RunRecord::new
//!
//! `sha2` is pure-Rust with no I/O, so this module keeps forge-domain
//! `wasm32-unknown-unknown`-clean.

use sha2::{Digest, Sha256};

/// The one canonical algorithm tag every `code_hash` must carry.
///
/// A `RunRecord.code_hash` (and any pipeline-emitted `Program.code_hash`) is
/// *only* well-formed if it starts with this literal prefix. Review 010 P1
/// flagged the divergence where the runtime recorded `fnv1a64:` while the
/// pipeline promised `sha256:`; exposing the prefix as a single constant lets
/// every crate assert against the *same* tag instead of hard-coding the string.
pub const CODE_HASH_PREFIX: &str = "sha256:";

/// Length of the hex body of a canonical hash: a SHA-256 digest is 32 bytes,
/// two lowercase-hex chars each.
pub const CODE_HASH_HEX_LEN: usize = 64;

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
    // prefix + two hex chars per digest byte (32 bytes -> 64).
    let mut out = String::with_capacity(CODE_HASH_PREFIX.len() + digest.len() * 2);
    out.push_str(CODE_HASH_PREFIX);
    for byte in digest {
        // Lowercase hex, two chars per byte — deterministic and platform-stable.
        // `unwrap` is on a constant 0..=15 nibble, never a real-path failure.
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out
}

/// True iff `s` is shaped like a value produced by [`code_hash`]: the literal
/// [`CODE_HASH_PREFIX`] followed by exactly [`CODE_HASH_HEX_LEN`] lowercase-hex
/// chars.
///
/// This is the contract teeth review 010 P1 was missing. Before this, the
/// canonical [`code_hash`] was a free function any crate could quietly *not*
/// adopt — the runtime kept recording `fnv1a64:` and nothing failed. Now any
/// recorder/replayer can reject a `code_hash` that is not the canonical
/// `sha256:` form (e.g. `fnv1a64:…`, an uppercase digest, or a truncated body)
/// instead of silently storing a divergent provenance string.
pub fn is_canonical_code_hash(s: &str) -> bool {
    let Some(hex) = s.strip_prefix(CODE_HASH_PREFIX) else {
        return false;
    };
    hex.len() == CODE_HASH_HEX_LEN
        && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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

    /// Every value `code_hash` produces is accepted by the canonical validator.
    #[test]
    fn code_hash_output_is_always_canonical() {
        for input in ["", "hello", "main()", "a longer applet body\nwith newlines"] {
            assert!(
                is_canonical_code_hash(&code_hash(input)),
                "code_hash({input:?}) must satisfy is_canonical_code_hash"
            );
        }
    }

    /// Regression for review 010 P1: the exact divergent form the runtime
    /// recorded (`fnv1a64:…`) is rejected, so a recorder that fails to adopt the
    /// canonical hash can be caught instead of silently storing it.
    #[test]
    fn fnv1a64_prefixed_hash_is_not_canonical() {
        assert!(!is_canonical_code_hash("fnv1a64:0123456789abcdef"));
    }

    /// Malformed canonical hashes are rejected: wrong prefix, no prefix,
    /// uppercase hex, short/long body, and non-hex characters.
    #[test]
    fn malformed_code_hashes_are_rejected() {
        let zeros = "0".repeat(CODE_HASH_HEX_LEN);
        // Wrong / missing prefix.
        assert!(!is_canonical_code_hash(&zeros), "no prefix");
        assert!(!is_canonical_code_hash(&format!("md5:{zeros}")), "wrong algo prefix");
        // Uppercase hex is not lowercase-canonical.
        assert!(
            !is_canonical_code_hash(&format!("{CODE_HASH_PREFIX}{}", "A".repeat(CODE_HASH_HEX_LEN))),
            "uppercase hex"
        );
        // Body length must be exactly CODE_HASH_HEX_LEN.
        assert!(
            !is_canonical_code_hash(&format!("{CODE_HASH_PREFIX}{}", "0".repeat(CODE_HASH_HEX_LEN - 1))),
            "short body"
        );
        assert!(
            !is_canonical_code_hash(&format!("{CODE_HASH_PREFIX}{}", "0".repeat(CODE_HASH_HEX_LEN + 1))),
            "long body"
        );
        // Non-hex character in an otherwise correctly sized body.
        assert!(
            !is_canonical_code_hash(&format!("{CODE_HASH_PREFIX}g{}", "0".repeat(CODE_HASH_HEX_LEN - 1))),
            "non-hex char"
        );
    }
}
