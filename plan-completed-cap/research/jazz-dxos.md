# Jazz + DXOS — local-first sync frameworks

The two most complete local-first application frameworks; the benchmark for
Terrane's multi-user cluster (crdt / sync-v2 / share-invite / presence).

## Jazz (jazz.tools)

- "The database that syncs": local-first relational DB spanning frontend,
  backend, and their sync/storage cloud; partial on-demand sync; files and
  durable streams built in ("eliminates external blob storage").
- **Groups & permissions:** CoValues belong to groups with reader/writer/admin
  roles; access control enforced by encryption signatures, no central backend;
  **invite tokens as URL-shareable secrets** (readerInvite/writerInvite).
- **Jazz 2.0 (2026):** moved *away* from purely cryptographic permission
  enforcement toward a trusted server applying richer policies at sync time;
  git-like snapshot history; relational/ORM query API.
- Local-first auth: self-signed tokens, optionally upgraded to an external
  provider (Better Auth / WorkOS via JWT) while preserving identity.

## DXOS

- P2P framework, no central servers: **ECHO** (replicated reactive DB with
  conflict resolution), **HALO** (decentralized identity via keypairs),
  **MESH** (WebRTC peer networking with a self-hostable signaling server).
- Data belongs to users, stored in *spaces* separate from any app — apps come
  and go, data stays. Flagship app Composer exports the whole database as
  plain markdown/json/media.

## What they validated for Terrane

- CRDT-per-app + recorded updates as the wire format (crdt cap, shipped).
- Invite-token sharing → [../cap-share-invite.md](../cap-share-invite.md).
- Jazz 2.0's retreat from crypto-enforced permissions to policy-at-sync-time
  is exactly [../cap-sync-v2.md](../cap-sync-v2.md)'s stance (edge policy over
  folded auth state, core stays policy-free) — independent convergence.
- Data-outlives-app (DXOS spaces) is Terrane's event-log position; Composer's
  export-everything → [../cap-backup-export.md](../cap-backup-export.md).

## What they exposed

- **User-level identity is the open residual:** HALO/Jazz auth give a durable
  cross-device *person*; Terrane has replica (device) identity + local auth
  members + Premium Google login, but no persistent user object that
  share-invite pairing attaches to. Named in the README residuals — likely a
  small `profile`/identity design inside the sync-v2/share slice.
- Presence/ephemeral messaging as a non-logged channel (both frameworks) →
  confirmed [../cap-presence-pubsub.md](../cap-presence-pubsub.md)'s
  deliberately-transient design.

## Sources

- https://jazz.tools/ and https://jazz.tools/blog/what-is-jazz
- https://github.com/garden-co/jazz
- https://dxos.org/ and https://docs.dxos.org/
