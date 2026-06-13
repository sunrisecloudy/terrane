---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/versioning-rollback.md, forge/fixtures/versioning/*.json, forge/fixtures/versioning/manifest.json
---

# T032 — Applet versioning & rollback spec + vectors (CR-7)

CR-7 (`prd-merged/01-core-runtime-prd.md` / docs equivalent): an installed applet
has versioned code, and the runtime can roll back to a prior version
deterministically. Since our code identity is the canonical `forge_domain::code_hash`
and runs are recorded/replayable (CR-8/9), versioning must compose with replay. I
want a spec + vectors before the Rust work.

## Deliverables

1. `forge/spec/versioning-rollback.md` — derive from the CR-7 PRD, the existing
   `forge_domain::code_hash` identity, and the install/run path in
   `forge/crates/core/src/workspace.rs`. Define: the applet version record
   (version id, code_hash, manifest hash, installed-at sequence), how install
   creates a new version, how rollback selects a prior version, the invariant that
   a replay of a run pinned to version V uses V's code_hash (not the current
   head), and what happens to data written under a newer version when rolling back
   (data is NOT destroyed; only the active code version changes) — call out the
   open question of schema/data compatibility across a rollback and propose the
   M-scope answer (rollback allowed only when the prior version's schema is
   compatible, else rejected with a clear error).

2. `forge/fixtures/versioning/<case>.json` + manifest — each: a sequence of
   install/upgrade/rollback ops and the expected active version + replay identity.
   Cover: install v1; upgrade to v2 (active=v2, v1 retained); rollback to v1
   (active=v1); a run recorded under v1 replays against v1's code_hash after an
   upgrade to v2; rollback to a non-existent version -> rejected; rollback across
   an incompatible schema change -> rejected with reason; re-upgrade after rollback
   -> active=v2 again; version list/history is ordered and complete.

## Coverage (~10)

install v1; upgrade v1->v2; rollback v2->v1; replay pinned to v1 after upgrade;
rollback to missing version rejected; rollback across incompatible schema rejected;
data written under v2 survives rollback to v1; history ordering; idempotent
re-install of same code_hash is a no-op (not a new version); active-version query.

In `## Result`, flag how rollback interacts with deterministic replay (a recorded
run must always resolve the code_hash it was recorded with, regardless of the
current active version).

## Result

(codex fills this in)
