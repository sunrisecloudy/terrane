---
status: done
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/dynamic-indexes.md, forge/fixtures/indexes/*.json, forge/fixtures/indexes/manifest.json
---

# T022 — Dynamic index lifecycle + expression-index vectors (DL-5/DL-6)

The data loop also needs dynamic indexes (prd-merged/02 DL-5): registry-declared
`indexed` fields create SQLite expression indexes (`json_extract(data,'$.<field_id>')`
partial per collection); full-text fields register FTS5 shadow tables; the index
lifecycle is `proposed → building → active → stale → rebuilding → deprecated →
removed` (resumable, idempotent), and the planner warns on full-scan fallback.

## Deliverables

1. `forge/spec/dynamic-indexes.md` — how indexes map onto the committed storage
   schema (read forge/crates/storage/src/ + forge/crates/schema/src/): the expression-
   index DDL the engine emits when a field is marked `indexed`, the FTS5 shadow table
   for text fields, the lifecycle states + transitions, the rebuild-from-canonical
   rule (DL-6), and when the planner must warn about a full scan.
2. `forge/fixtures/indexes/<case>.json` + manifest — each: a collection + a schema
   marking a field indexed, a set of records, a query, and the expected outcome
   markers: `uses_index` (true/false) and the result rows. Include: an indexed-field
   equality/range query that should use the index; a non-indexed field query that
   should warn full-scan; an FTS text query; an index added AFTER records exist
   (rebuild path); a deprecated index no longer used.

## Notes

This is spec + vectors, no Rust. The key open question to answer in `## Result`:
for the M0a/v1 subset, which lifecycle states are actually needed first
(proposed/active/rebuilding) vs deferrable, so I scope the Rust index manager to
what the vectors exercise rather than building the whole state machine up front.
