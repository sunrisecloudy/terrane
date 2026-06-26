---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/compat/*.json, forge/fixtures/compat/manifest.json
completed_at: 2026-06-13T05:33:58+07:00
---

# T006 — Forward-compatibility record fixtures (DL-9 / prd-merged/09 §3)

The normative rule (prd-merged/02 DL-9): a client must preserve fields and
collections it doesn't understand, and never strip them on read-modify-write.
prd-merged/09 §3 requires a versioned, never-deleted compatibility fixture suite.
I'll wire these into storage/schema/core round-trip tests.

## Deliverable

`forge/fixtures/compat/<case>.json`, each a `RecordEnvelope` (see
`forge/crates/domain/src/record.rs` for the exact shape: `envelope_version,
entity_id, collection, fields, field_ids, unknown_fields, extensions,
created_at, updated_at, deleted`) plus a `manifest.json` describing the expected
round-trip invariant.

## Cases (~12)

- A current-version record (all known fields).
- A "future" record carrying `unknown_fields` with stable ids this client can't
  resolve (e.g. `"f_future_3": {...}`) — must survive a patch that touches only a
  known field.
- A record in an **unknown collection** (DL-10) — must be storable/queryable as raw.
- A record with `extensions` (DL-13 reserved slot) populated — preserved verbatim.
- A record from a higher `envelope_version` — opened in limited mode, no data loss.
- A record where a field was renamed (same `field_ids["f0"]`, new display name) —
  old record still validates by field_id (mirrors prd-merged/02 DL-7).
- Edge: empty `fields` but populated `field_ids`; deleted=true tombstone record.

## manifest.json shape

```json
{ "cases": [
  { "file": "future_unknown_fields.json", "invariant": "preserve_on_patch",
    "patch": {"title": "edited"}, "must_keep_keys": ["f_future_3"],
    "note": "patching title must not drop the unknown stable field (DL-9)" }
] }
```

`invariant` ∈ `roundtrip_identical | preserve_on_patch | raw_queryable | limited_mode`.
In a `## Result`, list any case whose expected behavior you're unsure maps cleanly
to the current domain types so I can refine the envelope before these become CI gates.

## Result

Created 12 forward-compat `RecordEnvelope` fixture files plus
`forge/fixtures/compat/manifest.json`. The suite covers current v1 envelopes,
unknown future field ids, unknown collections, reserved `extensions`, higher
`envelope_version` limited-mode records, field renames via stable `field_ids`,
empty display fields with authoritative `field_ids`, tombstones, deprecated
fields, nested unknown values, and future schema/index metadata.

One mapping caveat for Claude: `prd-merged/02` names additional full-envelope
slots (`schema_id`, `schema_version`, `crdt`, `purge_policy`) that are not
fields on the current M0a `RecordEnvelope`; these fixtures keep to the exact
current Rust shape and model that future metadata through `unknown_fields` or
`extensions` until the domain type grows. `raw_queryable` and `limited_mode`
are manifest-level expectations for storage/schema tests rather than behavior
validated by the current record struct alone.
