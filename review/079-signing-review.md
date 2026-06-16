# Review 079 - forge-signing SC-15

Reviewed commit: `4ddc4f2c` (`forge-signing: app signing/trust (SC-15)`).

## Findings

1. **P2 - Per-file package hashes are accepted even when they lie.** MP-4 names package files as `files[{path, hash}]` (`prd-merged/08-marketplace-prd.md:18-20`), and `PackageFile::sha256` is documented as the publisher-recorded digest used for integrity (`forge/crates/signing/src/preimage.rs:20-31`). But `content_hash` recomputes `sha256(content)` and explicitly does not trust `PackageFile::sha256` (`forge/crates/signing/src/preimage.rs:140-164`), while `check_integrity` only compares the aggregate `contentHash` (`forge/crates/signing/src/trust.rs:128-156`). A package whose file bytes are intact but whose `files[].sha256` metadata is tampered to any bogus value still verifies as trusted. That leaves downstream install/audit code with untrusted file-digest metadata inside a trusted package. Add a check that every `PackageFile::sha256` equals the recomputed digest, plus a fixture where only `files[].sha256` is changed.

2. **P2 - The implemented content-hash preimage diverges from the stated docs/17 canonical form.** The commit and crate docs say this is the docs/17 `terrane/sig/v1` preimage, but docs/17 defines `contentHash` as sorted `(path "\n" SHA-256(file_bytes) "\n")` and requires path/line-ending/BOM canonicalization before hashing (`docs/17_APP_SIGNING_AND_TRUST.md:125-142`). The implementation hashes raw paths and raw string contents with a NUL separator (`forge/crates/signing/src/preimage.rs:140-164`), matching the new fixture README rather than docs/17. If another signer follows docs/17, valid packages will fail verification; if this verifier becomes canonical, path traversal / absolute-path / CRLF / BOM cases are not rejected or normalized here. Either update the normative signing spec and fixtures to the NUL form, or align the verifier with docs/17 and add fixtures for traversal, absolute paths, CRLF, and BOM normalization.

3. **P2 - Publisher trust expiry compares RFC3339 strings lexicographically.** `PublisherTrust::valid_until` is documented as an RFC3339 instant (`forge/crates/signing/src/trust.rs:111-125`), but `check_policy` rejects expiry with `if signed_at >= valid_until.as_str()` (`forge/crates/signing/src/trust.rs:177-181`). That only works for the fixture's fixed-width `Z` timestamps. RFC3339 permits offsets, so `signedAt = 2026-06-12T23:30:00Z` and `valid_until = 2026-06-13T00:00:00+01:00` should reject chronologically, but the string compare would allow it. Parse both timestamps as instants, reject malformed timestamps, and add offset/format regression cases.

## Verification

- Passed: `cargo test -p forge-signing`
- Passed: `cargo clippy -p forge-signing --all-targets -- -D warnings`
- Passed: `cargo check -p forge-signing --target wasm32-unknown-unknown --locked`
- Passed: `git show --check --format=short 4ddc4f2c`
- Passed: `git diff --check a49c34ad..4ddc4f2c`

No newer handoff files appeared after `T025`.
