# PRD 03 — Sync & Server (`forge-server`: managed cloud + embedded self-host)

**Status:** Merged draft v1 · **Depends on:** 01, 02 · **Depended on by:** 04, 06, 08
**Sources:** F-03 (home-server topology, cloud + embedded deployments, migration) + P-06 (handshake/message protocol, RBAC-validated sync, conflict policy) + decision D3 (home server; P2P transport rejected for v1)

## 1. Purpose

One Rust server crate, two deployments: (a) **managed cloud**, the hosted sync service; (b) the **same crate embedded inside the desktop app** (and as the Linux headless CLI), letting a user's own computer be the sync server for their devices, family, or team. Identical wire protocol; a workspace points at exactly one **home server** and can migrate between them. In M0, client and server run in one test process — sync is fully CI-testable from day one.

## 2. Protocol

- **SS-1** Transport: WebSocket (TLS 1.3), binary frames ≤ 256 KB, resumable. Per-document CRDT sync using Loro version-vector/frontier exchange; plus a presence channel (cursors, online members) and a control channel (membership, doc registry).
- **SS-2** Handshake (P-06): protocol version → peer identity + invite proof → feature capabilities → role claims → known frontiers → chunk request/response → ack → live subscribe. Message kinds: `hello, capabilities, frontier_summary, chunk_request/response, snapshot_offer/response, live_update, ack, conflict_notice, permission_denied, resync_required`.
- **SS-3** Frames are **transport-agnostic by design**: a future direct device-to-device pipe (v2 candidate) reuses the same frame types over a different transport. No WebRTC/P2P in v1 (decision D3).
- **SS-4** Offline-first: clients queue CRDT updates durably (DL `crdt_chunks`/oplog) and reconcile on reconnect; 1k pending ops converge p95 < 2 s.
- **SS-5** Forward compatibility: protocol version negotiated at handshake; unknown frame types skipped, unknown fields preserved (mirrors DL-9); servers older than N-2 minors show a must-update banner but continue to sync.

## 3. AuthZ: RBAC enforced at the server (merged F-SS-4 + P-06)

- **SS-6** Auth: workspace-scoped tokens. Cloud: optional account (email/OAuth/passkeys) → short-lived JWT per workspace. Embedded: device tokens via QR/short-code pairing; **no anonymous LAN access**. No login is ever required for local-only use (decision D8).
- **SS-7** Roles are **customizable RBAC** (P): defaults `Owner, Maintainer, Editor, Runner, Viewer, Auditor` (+ `Reviewer` for marketplace, PRD 08). Every remote operation is validated against actor identity, role, resource type, operation, capability grants, and schema compatibility **before application** — CRDT convergence is never a substitute for authorization. Rejections are logged and surfaced as `permission_denied` → client treats as a sync conflict in UI. Per-collection overrides v1.x.
- **SS-8** Membership: expiring role-scoped invite links; removal revokes tokens and triggers client purge of the workspace copy on next contact.
- **SS-9** Permission monotonicity (P-13 property test): removing a grant never increases access anywhere in the sync path.

## 4. Conflict policy (P-06)

- **SS-10** CRDT semantics resolve almost everything automatically. Conflict UI appears only for semantic ambiguity (dual rename, delete-vs-edit of a schema field, revoke-vs-dependent-run, uniqueness violations) and shows actors, logical clocks, affected resource, proposed auto-resolution, choices, and raw-op inspection.

## 5. Deployment A — managed cloud

- **SS-11** Stateless sync nodes (tokio), sessions sticky by `workspace_id`; Postgres for accounts/membership/metadata; object storage for snapshots/chunks/blobs; Redis presence fan-out.
- **SS-12** SLOs: 99.9% availability; op → other online client p95 < 500 ms intra-region; durable ack only after object-store write.
- **SS-13** Tenant isolation invariant-tested; cross-tenant access is SEV-1. Per-token rate limits, plan quotas, blob scanning on shared links, takedown workflow.
- **SS-14** Every workspace has an explicit, owner-chosen **server-visibility mode**, surfaced in UI and in the data model (normative; Review 001):
  - **Server-readable workspace (default):** enables workspace full-text search, read-only web share links, server-run LLM jobs at requester's scope, and scheduled applet tasks while devices are offline (opt-in).
  - **Encrypted workspace (project-level keys, DL-25):** payloads are ciphertext to the home server; the features above are **disabled or honestly degraded** (no server search/share/server LLM/offline scheduling; sync, membership, and relay still work). The mode and its trade-offs are shown at workspace creation and in settings; switching modes is owner-initiated and re-encrypts/migrates explicitly.

## 6. Deployment B — embedded server (desktop + Linux headless)

- **SS-15** Settings toggle "Use this computer as a server": in-process, configurable port, menubar/tray status, easy stop, visible address, access logs, role-based admission (P-10 desktop-as-server requirements).
- **SS-16** Reach: mDNS/Bonjour on LAN (advertises presence only, never grants access); remote access via outbound **relay tunnel** to the cloud relay (no port-forwarding; relay forwards TLS ciphertext) with direct-connection upgrade when possible. Self-hosters can disable relay entirely (LAN/VPN only).
- **SS-17** Optional roles of the embedded server (P-10): sync host, backup/export scheduler (DL-24 archives), local LLM gateway to LM Studio (PRD 04), local marketplace mirror (PRD 08), type-check service for paired thin clients.
- **SS-18** Availability honesty: syncs only while host is awake; clients display home-server status; sleep prevention opt-in. v1.x option: cloud ciphertext mailbox for offline embedded servers (flagged).
- **SS-19** Self-host packaging: same crate as a single binary + Docker image; SQLite first, Postgres at scale; admin UI; backup/restore (P-10).

## 7. Workspace migration (cloud ⇄ embedded)

- **SS-20** Owner-initiated, atomic: source seals read-only → ships snapshot set → target verifies frontiers → clients receive signed redirect → source tombstones. Mid-migration writes queue and replay. Migration soak in CI; cloud→embedded→cloud preserves byte-identical projection.

## 8. Security & ops (cross-ref PRD 07)

- **SS-21** TLS 1.3 everywhere incl. LAN (pinned self-signed cert exchanged at pairing — no TOFU); brute-force lockout on embedded; relay outbound-only.
- **SS-22** Observability: structured logs, RED metrics, per-doc sync-lag gauges; embedded local-only status page; **no document content in logs**; audit log of membership/admin events (cloud 1 yr; embedded local, configurable).

## 9. Acceptance

- 7-day soak: 50 workspaces, mixed cloud/embedded, randomized partitions/restarts/reordered+duplicated messages → zero divergence, zero acked-write loss.
- Embedded server on default desktop hardware serves 10 active members, p95 LAN sync < 100 ms; desktop hosts a browser client (M2 exit).
- Unauthorized remote ops rejected before application, logged, and surfaced; permission-monotonicity property tests green.
- M0: client↔server in-process round-trip with partition simulation green headlessly.

## 10. Open questions

1. Relay inclusion/pricing for Free-tier self-hosters.
2. Ciphertext mailbox (SS-18) timing.
3. Direct device-to-device transport: v2 evaluation criteria.
