---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/spec/sync-protocol.md, forge/fixtures/sync/*.json, forge/fixtures/sync/manifest.json
---

# T026 — In-process sync convergence fixtures (SS-1/2, M0b)

The next milestone is the in-process client↔server (peer↔peer) sync seam
(prd-merged/03 SS-1/2): two workspaces exchange CRDT updates so they converge to a
byte-identical projection — the local-first thesis. The CRDT/export foundation is
in place (records are CRDT-backed, `crdt_chunks` are append-only + immutable,
`rebuild_projection` reconstructs from chunks). I want a spec + convergence fixtures.

## Deliverables

1. `forge/spec/sync-protocol.md` — derive from the committed code (read
   `forge/crates/crdt/src/lib.rs` export_updates_since/import/from_updates,
   `forge/crates/storage/src/crdt_write.rs` chunk storage + rebuild,
   `prd-merged/03` SS-1/2): the M0b in-process sync model. The simplest faithful
   form: per CRDT doc, a peer advertises the chunk-ids it holds (its frontier); the
   other peer sends the chunks the first lacks (append-only, immutable, so order-
   independent); the receiver imports them + rebuilds the projection; do it both
   directions → converge. Note SS-7 (server-validates-remote-ops) as the next layer
   (M0b+), and that WebSocket transport is later (this is the in-process seam).

2. `forge/fixtures/sync/<case>.json` + manifest — each: peer A's record ops, peer B's
   record ops, and the EXPECTED converged projection both peers must reach:
   ```json
   { "case": "disjoint_collections_merge",
     "peer_a": [ {"insert": {"collection":"tasks","id":"t1","fields":{"title":"a"}}} ],
     "peer_b": [ {"insert": {"collection":"notes","id":"n1","fields":{"title":"b"}}} ],
     "expect_converged": { "tasks": [{"id":"t1","fields":{"title":"a"}}],
                           "notes": [{"id":"n1","fields":{"title":"b"}}] } }
   ```

## Coverage (~10)

disjoint collections merge; same collection different records merge; concurrent patch
to DIFFERENT fields of the same record (both survive, DL-3/9); concurrent write to the
SAME scalar field (deterministic LWW winner — note the winner is impl-defined but both
peers AGREE); a delete on one peer; A inserts then B syncs (one-directional catch-up);
empty peer syncs from a populated peer; both peers already in sync (no-op).

In `## Result`, flag any case whose converged state is ambiguous (esp. the same-scalar
LWW winner — state that both peers must agree but which value wins is impl-defined) so
the Rust sync test asserts agreement rather than a specific winner.
