---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/sync-envelope/*.json, forge/fixtures/sync-envelope/manifest.json
---

# T030 — Malformed / incomplete sync envelope validation vectors (SS-7, review 092 #2)

Codex review 092 #2 (P2) found that the wired sync apply path can allow a remote
chunk WITHOUT validating required document/resource/schema metadata: it stages
record envelopes with `schema_version: None`, reduces generic chunks to an Insert
with no `record_id`, and falls back to `collection = doc_id` for a non
`collection/<name>` doc id instead of rejecting. Per `forge/spec/sync-rbac.md:52`
and SS-7 (`prd-merged/03-sync-server-prd.md:21`) the receiver must fail closed on
missing/inconsistent metadata BEFORE any grant check or CRDT import.

We will make Rust staging fail closed. I need a vector suite that pins exactly
which envelopes are well-formed (may proceed to the grant check) vs malformed
(rejected pre-import), independent of role/grants.

## Deliverables

`forge/fixtures/sync-envelope/<case>.json` + manifest. Each case is an incoming
chunk envelope and the expected structural verdict (`well_formed` =>
proceeds to authorization; `rejected` => structurally invalid, denied before any
grant/CRDT work, with a reason). Keep these orthogonal to RBAC: assume a fully
trusted owner so the ONLY thing under test is envelope well-formedness.

```json
{ "case": "non_collection_doc_id_rejected",
  "incoming": { "doc_id": "blob/whatever",
    "metadata": { "resource_type": "record", "op": "insert", "record_id": "t1" } },
  "expect": { "verdict": "rejected", "reason_contains": "doc id must be collection/<name>" } }
```

## Coverage (~12)

- well-formed record insert (`collection/tasks`, op insert, record_id present,
  schema_version present) -> well_formed.
- well-formed schema change (resource_type schema, schema_id present,
  schema_version present) -> well_formed.
- doc id not of the form `collection/<name>` -> rejected.
- record op missing `record_id` -> rejected.
- record op missing `collection` (or collection inconsistent with the doc id's
  `<name>`) -> rejected.
- `resource_type` inconsistent with `op` (e.g. resource_type record but op
  schema_change, or vice versa) -> rejected.
- missing `schema_version` on a record write -> rejected.
- schema change missing `schema_id` -> rejected.
- schema_version present but not an integer / <= 0 -> rejected.
- a generic/multi-record chunk that cannot be reduced to a single concrete
  record identity -> rejected (do not silently coerce to Insert with empty id).
- collection name with disallowed characters / empty -> rejected.
- a duplicate/contradictory metadata (e.g. two different collections named in
  doc id vs metadata) -> rejected.

In `## Result`, note that these checks run BEFORE the RBAC grant decision and
BEFORE CRDT import, so a malformed envelope never mutates state, and that
`schema_version` presence is the hook for the later SS-7 schema-compatibility gate.

## Result

(codex fills this in)
