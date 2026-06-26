//! forge-signing: Ed25519 verification of applet / marketplace packages.
//!
//! Normative spec: prd-merged/07 **SC-15** (app signing/trust) and
//! prd-merged/08 **MP-4** (marketplace package format —
//! `files[{path, sha256}]`, `manifest`, `auth{signature}`). A package is
//! **Ed25519-signed**; the platform **verifies** the signature before it
//! trusts/installs the package. This crate is the verifier: it computes the
//! exact canonical preimage that was signed and checks the signature against
//! the publisher's public key.
//!
//! ## The canonical preimage (docs/17 `terrane/sig/v1`)
//!
//! The bytes that are signed are **not** the raw package JSON — they are a
//! short, line-oriented summary so the signature is stable across re-encodings
//! and so each tamperable region of the package gets its own hash. The exact
//! preimage (see [`package_preimage`]) is, joined by `\n` with **no** trailing
//! newline:
//!
//! ```text
//! terrane/sig/v1
//! <appId>
//! <appVersion>
//! <dataVersion>
//! <runtimeVersion>
//! <trustLevel>
//! <keyId>
//! <manifestHash>
//! <contentHash>
//! <permissionsHash>
//! <policyHash>
//! <signedAt>
//! ```
//!
//! The four hashes are `sha256:` + lowercase-hex (matching forge-domain's
//! `code_hash`) over, respectively:
//!
//! - **manifestHash** — stable key-sorted JSON of the *whole* manifest object;
//! - **contentHash** — the sorted file digest list: for each file, in `path`
//!   order, `path` + `NUL` + `sha256(content)` + `\n`;
//! - **permissionsHash** — stable key-sorted JSON of the `permissions` array;
//! - **policyHash** — stable key-sorted JSON of
//!   `{capabilities, networkPolicy, resourceBudget}`.
//!
//! Because the signature is over the *hashes*, the verifier separates three
//! failure layers (see [`FailureLayer`]):
//!
//! - **crypto** — the Ed25519 signature itself does not verify (wrong key,
//!   signature over different bytes, truncated/garbage/wrong-algorithm-label);
//! - **package_hash** — the signature verifies but a file/manifest/permissions/
//!   policy region was changed after signing, so the package no longer matches
//!   the hashes that were signed (the *integrity* layer);
//! - **policy** — the crypto + integrity are fine but the publisher is not
//!   trusted by *this* installer (unknown / expired trust). This is a
//!   marketplace-policy decision, not a cryptographic fact.
//!
//! ## Determinism / wasm
//!
//! `ed25519-dalek`, `sha2`, and `base64` are all pure-Rust with no I/O, so this
//! crate is `wasm32-unknown-unknown`-clean and the verifier is fully
//! deterministic.

#![forbid(unsafe_code)]

use forge_domain::{CoreError, Result};

mod preimage;
mod signature_payload;
mod token;
mod trust;
mod verify;

pub use preimage::{
    canonical_json, content_hash, file_digest, manifest_hash, package_preimage, permissions_hash,
    policy_hash, Package, PackageFile, PackageHashes, SIG_DOMAIN_TAG,
};
pub use signature_payload::{
    signature_payload, signature_payload_from_parts, SignaturePayloadFields,
};
pub use token::{
    encode_control_token, encode_control_token_from_entropy_b64, CONTROL_TOKEN_ENTROPY_BYTES,
};
pub use trust::{verify_package, FailureLayer, PublisherTrust, TrustError, TrustOutcome};
pub use verify::{verify_shell_signature, verify_signature, ALGORITHM_LABEL};

/// Re-export so callers can pattern-match the error this crate returns without
/// taking a direct dependency on forge-domain.
pub use forge_domain::CoreError as SigningError;

/// Construct the `ValidationError` this crate returns on every real-path
/// failure (bad signature / key / format / integrity / policy). Kept in one
/// place so the error kind is consistent and there is never a panic/unwrap on a
/// real path — SC-15 requires a typed result across the FFI boundary.
pub(crate) fn validation_error(msg: impl Into<String>) -> CoreError {
    CoreError::ValidationError(msg.into())
}

/// Convenience alias mirroring `forge_domain::Result`.
pub(crate) type SigResult<T> = Result<T>;
