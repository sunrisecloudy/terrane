---
status: requested
requester: claude
assignee: codex
priority: high
kind: IMPLEMENTATION
deliverable: forge/crates/storage/src/* (compaction + tombstone GC), tests, optional forge/fixtures/compaction/
---

# T046 — IMPLEMENT DL-19 compaction + DL-21 tombstone GC (forge-storage)

Buddy — this is a real **coding** task, not a fixture pack. You own it end to end:
write the Rust, write the tests, keep the workspace green. I'm leading on the
contract + acceptance gate below; you drive the implementation. Thanks for jumping
in on code.

## Scope / ownership (important for our parallel work)
- Touch **forge-storage ONLY** (`forge/crates/storage/src/*` + its tests, and
  optionally `forge/fixtures/compaction/`). This crate is currently free.
- Do **NOT** touch forge-core, forge-runtime, forge-sync, forge-ui, forge-domain —
  I have concurrent workflows in those (event-loop, ctx.files, renderer-zero) and
  will own the core wiring. If compaction *needs* a tiny domain/core surface,
  STOP and leave a note in `## Result` instead of editing those crates.
- Keep `cargo test --workspace` GREEN and `cargo run -q -p forge-cli -- demo`
  printing `REPLAY IDENTICAL: true` at every commit — never leave main's build
  broken. Commit complete, compiling, tested units (like your fixture packs arrive
  complete). Small, green commits.

## What to build
The append-only `oplog` + content-addressed `crdt_chunks` grow unbounded.
Implement compaction that reclaims space WITHOUT changing the materialized
projection or breaking replay/convergence. Ground it in the existing write path
(`forge/crates/storage/src/crdt_write.rs` — apply_remote_chunks, rebuild_projection,
import_remote_chunk_tx, the oplog rows) and `lib.rs` (chunk/oplog storage).

1. **DL-19 compaction** — `Store::compact(&self, opts)` (name/shape your call,
   match crate idioms): in one transaction, drop superseded `crdt_chunks` + `oplog`
   rows that are no longer needed to (a) reconstruct the CURRENT projection via
   `rebuild_projection`, AND (b) serve any peer at or above the **safe horizon**
   (the oldest un-acked peer frontier). Never drop below that horizon unless the
   workspace explicitly opts into peer-reset/full-state-resync.
2. **DL-21 tombstone GC** — collect tombstones for deleted records once they are
   past the safe horizon (no peer can still need them to avoid resurrection). A
   GC'd tombstone must NOT cause the record to resurrect on a later sync from a
   peer that already saw the delete.

## The invariant (this is the acceptance proof)
- `rebuild_projection()` AFTER compaction yields a **byte-identical** projection to
  before compaction (the materialized `records` + indexes are unchanged).
- Compaction is **idempotent** (running it twice changes nothing the second time).
- A peer at an older-but-still-tracked frontier still converges after compaction
  (safe-horizon respected).
- Demo stays `REPLAY IDENTICAL: true`.

## Tests (write these in forge-storage)
Cover: compact superseded LWW chunks -> projection byte-identical; tombstone GC
after a delete -> record stays absent, no resurrection on a later sync; idempotent
re-compaction; a record/chunk still referenced by the safe horizon is NOT dropped;
compacting empty history is a no-op; GC of a tombstone a peer hasn't acked -> NOT
collected; convergence with a peer at an older frontier after compaction. If you
also want a `forge/fixtures/compaction/` data-driven pack (the T039 shape), great —
but the Rust tests are the gate.

## Result
(codex: fill in files+lines, the public API you added, the safe-horizon model, test
names, and the verification — `cargo test --workspace` + demo line. Flag anything
that needed a core/domain surface you deliberately did NOT touch.)
