//! The canonical package preimage (docs/17 `terrane/sig/v1`) and the four
//! component hashes it is built from.
//!
//! Everything here is the *definition* of "what was signed". It must match the
//! T012 fixtures byte-for-byte: the fixtures carry both `signed_payload` (the
//! exact UTF-8 string) and `signed_payload_utf8_hex`, and a valid signature
//! verifies against these bytes, so the reconstruction below is checked end to
//! end by the data-driven tests.

use crate::{validation_error, SigResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The domain-separation tag that prefixes every signed payload. Signing a
/// distinct tag keeps a `terrane/sig/v1` signature from ever being mistaken for
/// a signature over some other forge byte string.
pub const SIG_DOMAIN_TAG: &str = "terrane/sig/v1";

/// One file in a package: its path, the verbatim content, and the per-file
/// `sha256:`-prefixed digest the publisher recorded (prd-merged/08 MP-4
/// `files[{path, hash}]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageFile {
    /// Logical path inside the package, e.g. `src/main.ts`.
    pub path: String,
    /// The verbatim file bytes (UTF-8 in the fixtures). Hashed to check
    /// integrity against [`PackageFile::sha256`] and the package `contentHash`.
    pub content: String,
    /// The per-file digest the publisher recorded: `sha256:` + lowercase-hex.
    pub sha256: String,
}

/// The recorded component hashes a package carries. These are the values that
/// were folded into the signed preimage; the verifier both *uses* them to build
/// the preimage and *recomputes* them from the live package to detect tamper
/// (the `package_hash` failure layer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHashes {
    #[serde(rename = "manifestHash")]
    pub manifest_hash: String,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
    #[serde(rename = "permissionsHash")]
    pub permissions_hash: String,
    #[serde(rename = "policyHash")]
    pub policy_hash: String,
}

/// A signed package: the manifest (an opaque JSON object so unknown fields are
/// preserved in the manifest hash), the files, and the recorded hashes.
///
/// The manifest is kept as a [`serde_json::Value`] on purpose: the
/// `manifestHash` is over *stable key-sorted JSON of the whole manifest*, so we
/// must round-trip every field — including ones this crate does not interpret —
/// or a valid package would fail its own integrity check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// The whole manifest object (prd-merged/08 MP-4 `manifest`).
    pub manifest: Value,
    /// The package files (prd-merged/08 MP-4 `files`).
    pub files: Vec<PackageFile>,
    /// The recorded component hashes folded into the signed preimage.
    pub hashes: PackageHashes,
}

/// Read a required string field out of the manifest, surfacing a typed
/// `ValidationError` (never a panic) if it is missing or not a string.
fn manifest_str<'a>(manifest: &'a Value, key: &str) -> SigResult<&'a str> {
    manifest
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| validation_error(format!("manifest.{key} is missing or not a string")))
}

/// `sha256:` + lowercase-hex over `bytes`. Identical algorithm and output shape
/// as `forge_domain::code_hash`, so every hash this crate emits is the canonical
/// forge content-hash form.
fn sha256_hex_prefixed(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        // Lowercase hex, two chars per byte. `unwrap` is on a constant 0..=15
        // nibble — never a real-path failure.
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out
}

/// Stable key-sorted, whitespace-free JSON of `value`.
///
/// `serde_json::Value` objects are backed by a `BTreeMap` (we do **not** enable
/// the `preserve_order` feature), so re-serializing already emits keys in sorted
/// order with no insignificant whitespace — exactly the canonical JSON the
/// fixtures hashed.
pub fn canonical_json(value: &Value) -> SigResult<String> {
    serde_json::to_string(value)
        .map_err(|e| validation_error(format!("manifest is not serializable to canonical JSON: {e}")))
}

/// `manifestHash` — `sha256:` over stable key-sorted JSON of the whole manifest.
pub fn manifest_hash(manifest: &Value) -> SigResult<String> {
    Ok(sha256_hex_prefixed(canonical_json(manifest)?.as_bytes()))
}

/// `permissionsHash` — `sha256:` over stable key-sorted JSON of the
/// `permissions` array. A missing array is treated as the empty array `[]` so a
/// permission-free manifest still has a stable hash.
pub fn permissions_hash(manifest: &Value) -> SigResult<String> {
    let permissions = manifest
        .get("permissions")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if !permissions.is_array() {
        return Err(validation_error("manifest.permissions is not an array"));
    }
    Ok(sha256_hex_prefixed(canonical_json(&permissions)?.as_bytes()))
}

/// `policyHash` — `sha256:` over stable key-sorted JSON of
/// `{capabilities, networkPolicy, resourceBudget}`. The three fields are pulled
/// out of the manifest and placed in a fresh object; because the object is
/// re-serialized in sorted-key order the field order here does not matter.
pub fn policy_hash(manifest: &Value) -> SigResult<String> {
    let mut policy = serde_json::Map::new();
    for key in ["resourceBudget", "networkPolicy", "capabilities"] {
        let field = manifest
            .get(key)
            .cloned()
            .ok_or_else(|| validation_error(format!("manifest.{key} is missing (policy hash)")))?;
        policy.insert(key.to_string(), field);
    }
    Ok(sha256_hex_prefixed(
        canonical_json(&Value::Object(policy))?.as_bytes(),
    ))
}

/// The canonical per-file digest: `sha256:` + lowercase-hex over the verbatim
/// file `content`. This is the value a publisher records in
/// [`PackageFile::sha256`] (prd-merged/08 MP-4 `files[{path, hash}]`); the
/// verifier recomputes it both to fold into [`content_hash`] and to confirm the
/// recorded per-file digest does not lie (review 079 #1).
pub fn file_digest(content: &str) -> String {
    sha256_hex_prefixed(content.as_bytes())
}

/// `contentHash` — `sha256:` over the sorted file-digest list. For each file,
/// in ascending `path` order, the running buffer gets:
///
/// ```text
/// <path> NUL <sha256(content)> \n
/// ```
///
/// where `<sha256(content)>` is the `sha256:`-prefixed digest of the verbatim
/// file content (i.e. the canonical per-file digest, recomputed here rather than
/// trusting the recorded [`PackageFile::sha256`]).
pub fn content_hash(files: &[PackageFile]) -> String {
    let mut entries: Vec<(&str, String)> = files
        .iter()
        .map(|f| (f.path.as_str(), file_digest(&f.content)))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut buf: Vec<u8> = Vec::new();
    for (path, digest) in entries {
        buf.extend_from_slice(path.as_bytes());
        buf.push(0); // NUL separator
        buf.extend_from_slice(digest.as_bytes());
        buf.push(b'\n');
    }
    sha256_hex_prefixed(&buf)
}

/// Build the **exact** bytes that were signed for `pkg` — the docs/17
/// `terrane/sig/v1` preimage.
///
/// The lines are joined with `\n` and there is **no** trailing newline after
/// `signedAt`. The four hashes come from the package's *recorded* hashes (so the
/// preimage equals what the publisher signed); separate integrity checks in
/// [`crate::verify_package`] confirm those recorded hashes still match the live
/// package.
pub fn package_preimage(pkg: &Package) -> SigResult<Vec<u8>> {
    let m = &pkg.manifest;
    let lines = [
        SIG_DOMAIN_TAG,
        manifest_str(m, "appId")?,
        manifest_str(m, "appVersion")?,
        manifest_str(m, "dataVersion")?,
        manifest_str(m, "runtimeVersion")?,
        manifest_str(m, "trustLevel")?,
        manifest_str(m, "keyId")?,
        pkg.hashes.manifest_hash.as_str(),
        pkg.hashes.content_hash.as_str(),
        pkg.hashes.permissions_hash.as_str(),
        pkg.hashes.policy_hash.as_str(),
        manifest_str(m, "signedAt")?,
    ];
    Ok(lines.join("\n").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str, content: &str) -> PackageFile {
        PackageFile {
            path: path.into(),
            content: content.into(),
            sha256: sha256_hex_prefixed(content.as_bytes()),
        }
    }

    #[test]
    fn sha256_form_is_prefixed_lowercase_hex() {
        let h = sha256_hex_prefixed(b"");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h.len(), "sha256:".len() + 64);
        assert!(h
            .strip_prefix("sha256:")
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    #[test]
    fn content_hash_is_order_independent_over_files() {
        let a = file("a.ts", "A\n");
        let b = file("b.ts", "B\n");
        let ordered = content_hash(&[a.clone(), b.clone()]);
        let reversed = content_hash(&[b, a]);
        assert_eq!(ordered, reversed, "files are sorted by path before hashing");
    }

    #[test]
    fn canonical_json_sorts_keys_without_spaces() {
        let v = json!({ "b": 1, "a": 2 });
        assert_eq!(canonical_json(&v).unwrap(), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn permissions_hash_treats_missing_as_empty_array() {
        let none = json!({});
        let empty = json!({ "permissions": [] });
        assert_eq!(
            permissions_hash(&none).unwrap(),
            permissions_hash(&empty).unwrap()
        );
    }

    #[test]
    fn policy_hash_is_field_order_independent_in_the_manifest() {
        let m1 = json!({
            "resourceBudget": { "wall_ms": 1 },
            "networkPolicy": { "allow": [] },
            "capabilities": { "ui": true }
        });
        let m2 = json!({
            "capabilities": { "ui": true },
            "networkPolicy": { "allow": [] },
            "resourceBudget": { "wall_ms": 1 }
        });
        assert_eq!(policy_hash(&m1).unwrap(), policy_hash(&m2).unwrap());
    }

    #[test]
    fn missing_preimage_field_is_a_typed_error_not_a_panic() {
        let pkg = Package {
            manifest: json!({ "appId": "x" }), // missing the rest
            files: vec![],
            hashes: PackageHashes {
                manifest_hash: String::new(),
                content_hash: String::new(),
                permissions_hash: String::new(),
                policy_hash: String::new(),
            },
        };
        let err = package_preimage(&pkg).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn preimage_starts_with_the_domain_tag_and_has_no_trailing_newline() {
        let pkg = Package {
            manifest: json!({
                "appId": "app", "appVersion": "1", "dataVersion": "1",
                "runtimeVersion": "rt", "trustLevel": "t", "keyId": "k",
                "signedAt": "2026-01-01T00:00:00Z"
            }),
            files: vec![],
            hashes: PackageHashes {
                manifest_hash: "sha256:aa".into(),
                content_hash: "sha256:bb".into(),
                permissions_hash: "sha256:cc".into(),
                policy_hash: "sha256:dd".into(),
            },
        };
        let bytes = package_preimage(&pkg).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with(&format!("{SIG_DOMAIN_TAG}\n")));
        assert!(s.ends_with("2026-01-01T00:00:00Z"));
        assert!(!s.ends_with('\n'));
    }
}
