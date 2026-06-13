---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/audit-log.md, forge/fixtures/audit-log/*.json, forge/fixtures/audit-log/manifest.json
---

# T031 — Audit log persistence spec + vectors (SC-12)

We now emit audit decisions in several places (command RBAC, sync-RBAC remote-op
denials/allows). SC-12 (`prd-merged/07-security-prd.md`) requires denials (and
notable allows) to be persisted as an append-only, queryable audit log. Before we
build the Rust persistence, I want a spec + vectors locking the record shape,
ordering, and query contract.

## Deliverables

1. `forge/spec/audit-log.md` — derive from prd-merged/07 (SC-12) and the existing
   audit shapes already produced by `forge/crates/core` (grep for the current
   audit/denial structs — sync-RBAC in `forge/crates/core/src/sync_rbac.rs` and
   the command path). Define: the canonical audit record fields (timestamp source
   — note determinism constraint, monotonic sequence number, actor id, action,
   resource type, collection/schema id, decision allow|deny, trusted role,
   trusted grants snapshot, reason); append-only semantics (no update/delete);
   the deterministic ordering key; and a minimal query surface (filter by actor /
   action / decision / resource, time/sequence range).

2. `forge/fixtures/audit-log/<case>.json` + manifest — each: a sequence of emitted
   audit events and an expected persisted/queried result. Cover: a deny is
   persisted with all required fields; an allow is persisted; ordering is stable
   by sequence; a query by actor returns only that actor's events; a query by
   decision=deny returns only denials; the log is append-only (no event mutates a
   prior one); a redaction rule if any field is sensitive (e.g. do not store secret
   values, only secret_ref ids).

## Coverage (~10)

required-fields-present deny; allow persisted; sequence ordering; filter by actor;
filter by action; filter by decision; filter by resource/collection; append-only
invariant (re-emitting does not rewrite history); empty-result query; a
sensitive-field redaction case.

In `## Result`, flag the timestamp determinism decision (audit time must not break
deterministic replay — propose recording a logical sequence + an externally
supplied wall clock rather than calling the clock inside the replayable path).

## Result

(codex fills this in)
