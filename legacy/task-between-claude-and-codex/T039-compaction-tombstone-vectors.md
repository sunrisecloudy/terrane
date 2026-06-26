---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/compaction.md, forge/fixtures/compaction/*.json, forge/fixtures/compaction/manifest.json
---

# T039 — DL-19 compaction + DL-21 tombstone GC vectors

The oplog + content-addressed crdt_chunks grow unbounded. DL-19 (compaction) and DL-21
(tombstone delete/GC) reclaim space WITHOUT changing the materialized projection or
breaking replay/convergence. Spec + vectors before the Rust work.

## Deliverables
1. `forge/spec/compaction.md` — derive from prd-merged/02 (DL-19/DL-21), the append-only
   oplog + crdt_chunks + projection rebuild (forge/crates/storage/src/crdt_write.rs). Define:
   what compaction may drop (superseded chunks/oplog rows no longer needed to reconstruct the
   current projection + required history horizon), the invariant that rebuild_projection after
   compaction yields a byte-identical projection, tombstone GC for deleted records (DL-21:
   when a tombstone may be collected without resurrecting the record on a later sync), and
   the determinism/convergence constraints (compaction must not break a peer that hasn't seen
   the compacted history — flag the safe horizon rule).
2. `forge/fixtures/compaction/<case>.json` + manifest. Each: a chunk/oplog history, a
   compaction op, and the expected retained set + post-compaction projection.

## Coverage (~10)
compact superseded LWW chunks -> projection unchanged; tombstone GC after a delete ->
record stays absent, no resurrection; compaction is idempotent; rebuild after compaction is
byte-identical; a record still referenced by the history horizon is NOT dropped; compaction
preserves convergence with a peer at an older frontier (safe horizon); compacting an empty
history is a no-op; GC of a tombstone that a peer hasn't acked -> NOT collected.

In `## Result`, flag the safe-horizon rule (do not compact below the oldest un-acked peer
frontier) so compaction never breaks convergence.

## Result
(codex fills this in)
