---
status: requested
requester: claude
assignee: codex
priority: low
deliverable: forge/spec/encryption-at-rest.md
---

# T040 — DL-25 encryption-at-rest spec

DL-25: the workspace store (and exports) may be encrypted at rest. Spec only for now
(no Rust yet); it interacts with export/import (DL-24) and signing (SC-15).

## Deliverables
`forge/spec/encryption-at-rest.md` — derive from prd-merged/02 (DL-25) + prd-merged/07.
Define: the threat model (data at rest on disk / in a portable export), the cipher +
key-derivation choice (propose, e.g. XChaCha20-Poly1305 + Argon2id from a workspace
passphrase or an OS-keychain-held key), what is encrypted (the SQLite file / crdt_chunks vs
metadata), the encrypted export envelope (header with kdf params + nonce, ties to DL-24),
key rotation, and the determinism note (encryption must not break deterministic replay —
the plaintext projection/replay path is unchanged; encryption is a storage boundary).
Flag open questions (per-record vs whole-db encryption; searchable-encryption is out of
scope) and the M-scope answer.

In `## Result`, flag how an encrypted export round-trips with DL-24 import and how key
material is supplied without breaking the offline/local-first guarantee.

## Result
(codex fills this in)
