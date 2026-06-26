---
status: done
requester: claude
assignee: codex
priority: low
deliverable: forge/fixtures/signing/*.json, forge/fixtures/signing/manifest.json, forge/fixtures/signing/README.md
---

# T012 — App signing / trust test vectors (Ed25519; docs/17 / SC-15 / MP-4)

prd-merged/07 SC-15 + prd-merged/08 MP-4: applet/marketplace packages are
Ed25519-signed (signing-ready format; enforcement later). docs/17_APP_SIGNING_AND_TRUST.md
has the v0.4 trust model — port the *crypto contract*, not the v0.4 install pipeline.
These vectors drive the future `signing` verification path.

## Deliverable

`forge/fixtures/signing/` with: a fixed test keypair (public + private, clearly
labeled TEST-ONLY in the README), several signed package manifests, and tamper
cases, plus `manifest.json` listing expected `valid|invalid` + reason.

```json
{ "case": "valid_signature",
  "package": { "files": [{"path":"src/main.ts","sha256":"..."}], "manifest": {...} },
  "signature": "ed25519:...", "public_key": "ed25519:...",
  "expect": "valid" }
```

## Coverage (~14)

Valid: correctly signed package; multi-file package with per-file hashes.
Invalid: wrong key; signature over different bytes; one file's content changed after
signing (hash mismatch); manifest field changed after signing; truncated/garbage
signature; right signature but wrong algorithm label; expired/unknown publisher
(MP-5 provenance) — mark as "policy" vs "crypto" failure.

Use real Ed25519 (you may generate with any standard tool); include the exact bytes
that were signed for each case so the Rust verifier is reproducible. In `## Result`,
state the canonical-bytes definition you signed over (e.g. canonical-JSON of which
fields) so the Rust side hashes/signs the identical preimage.

## Result

Created `forge/fixtures/signing/` with a TEST-ONLY deterministic Ed25519 keypair, 14 signed/tampered package vectors, `manifest.json`, and `README.md`. Valid vectors include a single-file and multi-file package. Invalid vectors cover wrong key, signature over different bytes, changed file content, changed manifest, truncated/garbage signature, wrong algorithm label, unknown/expired publisher policy, policy hash mismatch, permissions hash mismatch, and public-key mismatch.

Canonical bytes are documented in the README: the docs/17 `terrane/sig/v1` newline payload with appId, appVersion, dataVersion, runtimeVersion, trustLevel, keyId, manifestHash, contentHash, permissionsHash, policyHash, and signedAt, with no trailing newline. Each case includes the signed payload and UTF-8 hex preimage.
