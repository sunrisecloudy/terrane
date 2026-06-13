---
status: done
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/workspace-export-format.md, forge/fixtures/export/*.json
---

# T017 — Workspace export/import format spec + fixtures (DL-24; docs/29)

prd-merged/02 DL-24: a workspace exports as a single SQLite file (metadata, applet
sources, schemas, records, CRDT snapshots/chunks, oplog, index defs, RBAC config;
run logs per policy), and re-import reproduces a byte-identical projection. It's also
the backup format and the public/open format spec (GA). docs/29_BACKUP_EXPORT_IMPORT.md
has the v0.4 detail. I want the v1 format spec + small fixtures.

## Deliverable

1. `forge/spec/workspace-export-format.md` — the canonical export contents mapped to
   the committed `forge-storage` physical schema (read `forge/crates/storage/src/`):
   which tables/sections are included, ordering rules for determinism, what's excluded
   (local-only settings, secrets — never exported), versioning of the format, and the
   re-import "byte-identical projection" invariant (DL-6 rebuild interplay).
2. `forge/fixtures/export/` — a couple of small, hand-checkable export descriptors:
   a JSON manifest of what a tiny workspace's export should contain (collections,
   record count, doc ids) so an export/import round-trip test can assert structure.

In `## Result`, flag anything the current storage schema doesn't yet persist that the
export needs (e.g. RBAC config rows) so I extend storage before wiring export.

## Result

Created `forge/spec/workspace-export-format.md` and `forge/fixtures/export/`. The spec maps DL-24 to the committed `forge-storage` tables (`meta`, `kv`, `oplog`, `crdt_chunks`, `crdt_snapshots`, `records`, `run_logs`, `runs`), defines deterministic ordering, exclusions, versioning, and the byte-identical projection re-import invariant.

Current storage gaps called out for Claude: applet sources, applet manifests/signatures, schema registry CRDT document, index definitions, RBAC config, permissions, and marketplace provenance do not yet have dedicated persisted rows. Fixtures include tiny workspace, debug/run-log inclusion, and redacted-secret descriptors.
