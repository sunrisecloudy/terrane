---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/migrations.md, forge/fixtures/migrations/*.json, forge/fixtures/migrations/manifest.json
---

# T033 — Schema migration spec + vectors (DL-13)

DL-13 (`prd-merged/02-data-layer-prd.md`): schemas evolve and existing records
must migrate deterministically. We already have schema commands (DL-7/8:
`schema.apply_change`/`validate`/`rebuild_indexes`) and CRDT-backed records with a
projection rebuild. Migrations sit on top: a schema change that alters field shape
needs a deterministic record transform. Spec + vectors before the Rust work.

## Deliverables

1. `forge/spec/migrations.md` — derive from DL-13, the committed schema registry
   (`forge/crates/schema`, the schema commands in `forge/crates/core`), and the
   CRDT projection rebuild (`forge/crates/storage/src/crdt_write.rs`). Define: the
   migration descriptor (from schema_version -> to schema_version, per-field
   transforms: add with default, rename, drop, type-coerce), determinism (same
   inputs -> identical migrated records, replay-safe, content-addressed), how a
   migration composes with the projection rebuild (migrate at materialization or
   as a recorded op — propose and justify), forward-only vs reversible, and the
   failure mode when a record cannot be migrated (reject the migration atomically,
   leave data untouched — mirrors review 088 atomic apply).

2. `forge/fixtures/migrations/<case>.json` + manifest — each: a starting schema +
   records, a migration descriptor, and the expected migrated records (or a
   rejection). Cover: add field with default; rename field (values preserved);
   drop field; widen type (int->string) deterministically; a migration that fails
   on one record rolls back entirely; idempotent re-apply of an
   already-applied migration is a no-op; migration bumps schema_version; records
   written before migration read back in the new shape after rebuild.

## Coverage (~10)

add-with-default; rename; drop; type-widen; per-record transform determinism;
atomic rollback on a bad record; idempotent re-apply; schema_version bump;
projection rebuild reflects migrated shape; a no-op migration (identical schemas).

In `## Result`, flag the determinism decision (a migration must be a pure function
of the prior record + descriptor so replay and content-addressing hold) and
whether migrations are recorded in the oplog as ops (so remote peers converge).

## Result

(codex fills this in)
