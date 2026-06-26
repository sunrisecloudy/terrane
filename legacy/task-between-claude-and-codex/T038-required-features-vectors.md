---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/required-features.md, forge/fixtures/required-features/*.json, forge/fixtures/required-features/manifest.json
---

# T038 — MP-8 required_features / capability negotiation vectors

The audit found MP-8 (packages declare required features; clients refuse unsupported ones)
entirely missing — it blocks graceful package compat and connects to the signing
fail-closed work (reviews 086/089). Spec + vectors before wiring.

## Deliverables
1. `forge/spec/required-features.md` — derive from prd-merged/08 (MP-8) and the signed-policy
   fail-closed bind (forge/crates/core/src/workspace.rs, reviews 086/089). Define the
   `required_features` manifest field (a list of capability/runtime feature ids + min
   versions), the client feature registry, and the rule: install ONLY if the client supports
   every required feature; otherwise refuse with a clear, enumerated unsupported list. Tie to
   the signed-package unknown-field fail-closed (a signed future feature must be declared in
   required_features for this client to accept it).
2. `forge/fixtures/required-features/<case>.json` + manifest. Each: a package's
   required_features + the client's supported set, and expected install/refuse.

## Coverage (~10)
all required supported -> install; one unsupported feature -> refuse naming it; required
min-version higher than client -> refuse; empty required_features -> install; an unknown
signed policy field declared in required_features that the client supports -> install; same
field NOT declared -> refuse (ties to review 086); multiple unsupported -> refuse listing
all; case/normalization of feature ids; forward-compat (client supports a superset).

In `## Result`, flag how required_features composes with the signed-package unknown-field
fail-closed gate so the two agree.

## Result
(codex fills this in)
