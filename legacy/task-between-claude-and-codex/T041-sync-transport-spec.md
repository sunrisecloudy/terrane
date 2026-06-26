---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/sync-transport.md, forge/fixtures/sync-transport/*.json, forge/fixtures/sync-transport/manifest.json
---

# T041 — SS transport handshake / framing spec (M2)

The in-process sync kernel (SS-1/2) + RBAC (SS-7) is proven, but there is no transport.
SS-3 (transport-agnostic frames) + the M2 WebSocket transport need a wire spec before the
Rust networking work. Spec + protocol-level vectors (no real sockets).

## Deliverables
1. `forge/spec/sync-transport.md` — derive from prd-merged/03 (SS-3/SS-6/SS-1/2) and the
   existing in-process SyncOpEnvelope (forge/crates/sync/src/lib.rs) + sync-protocol.md.
   Define the session handshake (peer identity + auth token presentation; receiver resolves
   trusted membership — ties to SS-7), the frame types (hello, have/want frontier exchange,
   chunk_response, live_update, ack, error/permission_denied), framing/length encoding,
   reconnect + resume from a frontier, and backpressure. Keep authorization at apply time
   (SS-7) — the transport never widens trust.
2. `forge/fixtures/sync-transport/<case>.json` + manifest. Each: a scripted frame exchange
   and the expected response frames / resulting frontier (transport-level, no CRDT bytes).

## Coverage (~10)
handshake success -> session established; handshake with an untrusted/invalid token ->
rejected; frontier exchange computes the missing-chunk want set; chunk_response advances the
frontier; reconnect resumes from the last acked frontier (no re-send of acked chunks);
an unauthorized remote op frame -> permission_denied frame (SS-7), session stays open;
malformed frame -> error frame, no state change; live_update delivers an incremental change;
duplicate/replayed frame is idempotent.

In `## Result`, flag that the transport is authorization-neutral (SS-7 decides at apply
time) and how resume avoids re-sending acked chunks.

## Result
(codex fills this in)
