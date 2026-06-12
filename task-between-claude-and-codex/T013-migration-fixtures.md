---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/fixtures/migrations/*.json, forge/fixtures/migrations/manifest.json
---

# T013 — Schema migration sequence fixtures (DL-8/DL-13; docs/19, docs/28)

prd-merged/02 DL-8 (additive-only evolution) + DL-13 (migrations are oplog ops, not
destructive DDL). docs/19_DATA_MIGRATIONS.md + docs/28 have the v0.4 detail. I want
fixtures that drive the schema-registry evolution + the future migration path. These
align with the actual `forge-schema` API (read `forge/crates/schema/src/`).

## Deliverable

`forge/fixtures/migrations/<case>.json` = an ordered list of `SchemaChange` ops + the
expected final registry state (or an expected rejection), plus `manifest.json`.

```json
{ "case": "add_then_widen",
  "changes": [
    { "AddCollection": { "name": "tasks" } },
    { "AddField": { "collection": "tasks", "name": "priority", "ty": "IntNum" } },
    { "WidenField": { "collection": "tasks", "field": "priority", "to": "FloatNum" } }
  ],
  "expect": "ok",
  "final": { "collections": [ { "name": "tasks", "fields": ["..."] } ] } }
```

(Match the real SchemaChange variant names in the committed forge-schema crate — read
it first; if names differ, follow the crate and note it.)

## Coverage (~14)

OK: add collection → add fields (stable ids) → rename (name change, id kept) →
deprecate field → widen Int→Float → add index.
Rejected (additive-only / DL-8): narrow Float→Int; re-add existing collection; reuse
a field id; any destructive remove; new required constraint applied in enforce mode
before warn mode (DL-12).
Forward-compat: a sequence by two different actors (actor-scoped ids per DL-11 —
the schema crate is gaining actor-scoped ids now) that MERGES to the union without id
collision.

`expect` ∈ `ok | rejected`. In `## Result`, note any change type the current
SchemaChange enum can't express yet so I can extend it.
