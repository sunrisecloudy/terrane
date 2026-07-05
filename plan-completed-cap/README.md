# Capability plans

Specs + implementation plans for every capability Terrane still needs to be a
general-purpose app platform. Each file is self-contained: purpose, surface
tables (commands/events/resources), replay story, security, limits, phased
implementation plan, and non-goals.

Two statuses:

- **Locked** — decisions confirmed by the user (2026-07-05); ready to build.
- **Draft** — full spec written against the recommended common solution; each
  ends with a "Decisions to confirm" section listing the choices that are the
  user's to veto before implementation.

Already shipped, so no plan needed: `search` (hybrid search, merged),
`local-model.embed` (merged with search). Deliberately skipped for now:
payments.

The roadmap was audited against Sandstorm, Urbit, Val Town, Tauri, Jazz/DXOS,
the ChatGPT Apps SDK, and Cloudflare Workers — source notes and the coverage
matrix live in [research/](research/README.md).

## Index

### Primitives — the level above capabilities

| Plan | What it is | Status | Depends on |
| --- | --- | --- | --- |
| [primitive-actor.md](primitive-actor.md) | required `actor` on every event (engine change + one-time log migration) | **Locked** | nothing — do **early**, before data accumulates |
| [primitive-person.md](primitive-person.md) | durable identity = local ed25519 keypair + attestations | **Locked** | connection keychain |
| [primitive-org.md](primitive-org.md) | org = shared home; Premium hosting as convenience | **Locked** | person, sync v2, share-invite, actor |
| [primitive-item.md](primitive-item.md) | `terrane://app/<id>/item/<itemId>`; `items` interface required on every app | **Locked** | interop rollout (same validation slice) |

### Foundation — data & core surface

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-blob.md](cap-blob.md) | `blob` | **Locked** | — |
| [cap-net-v2.md](cap-net-v2.md) | `net` (extended) | **Locked** | blob |
| [cap-query.md](cap-query.md) | `query` | **Locked** | kv, relational_db |
| [cap-document.md](cap-document.md) | `document` | Draft | — (names frozen by the live host's planned entry) |
| [cap-time.md](cap-time.md) | `time` | Draft | — |
| [cap-telemetry.md](cap-telemetry.md) | `telemetry` | Draft | js-runtime |
| [cap-history.md](cap-history.md) | `history` | Draft | the log itself; compaction horizon |

### App-to-app composition

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-interop.md](cap-interop.md) | `interop` | **Locked** | js-runtime, auth elicitation; `common.receive` **required on every app** |
| [cap-deep-links.md](cap-deep-links.md) | `app` (extended) | Draft | interop (`common.receive`), blob |

### Background work

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-scheduler.md](cap-scheduler.md) | `scheduler` | Draft | time, js-runtime, a long-running host |
| [cap-job-queue.md](cap-job-queue.md) | `job` | Draft | scheduler |
| [cap-automation.md](cap-automation.md) | `automation` | Draft | query (JMESPath), scheduler's host loop; push should converge onto it |

### Outbound & inbound integration

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-oauth-connections.md](cap-oauth-connections.md) | `connection` | Draft | net v2 (fulfils its `$secret` reservation), crypto primitives, OS keychain |
| [cap-webhook.md](cap-webhook.md) | `webhook` | Draft | blob, net v2 redaction rules |
| [cap-stream.md](cap-stream.md) | `stream` | Draft | net v2, blob; log growth ties to compaction |
| [cap-common.md](cap-common.md) | `common` (`common.send`, channels; email first) | Draft (**name + channel model: locked**) | connection, blob; **v2 receive rides interop** (user-confirmed) |
| [cap-mcp-client.md](cap-mcp-client.md) | `mcp` | Draft | connection, blob |
| [cap-web-publish.md](cap-web-publish.md) | `web-publish` | Draft (**Premium-gated: locked**) | Premium relay, connection keychain |
| [cap-browser.md](cap-browser.md) | `browser` | Draft | blob, net-v2 policies; mac WKWebView / chromium fallback |

### Multi-user

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-sync-v2.md](cap-sync-v2.md) | `sync` + host transport | Draft | crdt, replica; blob pass |
| [cap-share-invite.md](cap-share-invite.md) | `share` | Draft | sync v2, auth |
| [cap-presence-pubsub.md](cap-presence-pubsub.md) | `presence` | Draft | sync v2 transport (deliberately transient — messages never hit the log) |
| [cap-push.md](cap-push.md) | `push` | Draft | sync v2, native |

### Media & devices

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-media.md](cap-media.md) | `media` | Draft | blob |
| [cap-capture.md](cap-capture.md) | native ops | Draft | native, blob |
| [cap-tts.md](cap-tts.md) | `tts` | Draft | blob (render path) |
| [cap-geolocation.md](cap-geolocation.md) | `geo` | Draft | — |
| [cap-native-v2.md](cap-native-v2.md) | `native` (extended) | Draft | blob (screen.capture); promotes existing planned catalog stubs |
| [cap-applescript.md](cap-applescript.md) | `applescript` | Draft | **crate already exists** on branch `feat/mac-control-applescript-dual-mlx` — plan is extraction, not construction |

### AI & platform lifecycle

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-model-v2.md](cap-model-v2.md) | `model` + `local-model` (extended) | Draft | blob (image parts); connection for direct API providers (decision) |
| [cap-schema-migration.md](cap-schema-migration.md) | `migration` | Draft | js-runtime |
| [cap-app-update.md](cap-app-update.md) | `app` (extended) | Draft | schema-migration, builder drafts, blob |
| [cap-publish.md](cap-publish.md) | `publish` | Draft | replica, connection keychain, app-update |

### Engine & operations (not capability crates)

| Plan | Scope | Status | Depends on |
| --- | --- | --- | --- |
| [cap-compaction.md](cap-compaction.md) | terrane-core + terrane-host | Draft | resolves the blob-GC race; sync retain-window |
| [cap-backup-export.md](cap-backup-export.md) | terrane-host + CLI | Draft | blob (conditional), replica |

## Locked decisions (user, 2026-07-05)

1. **Blob bytes = content-addressed sidecar.** Events carry `{hash, size, mime}`
   only; bytes live in a SQLite blob table keyed by SHA-256. The log alone
   rebuilds all *state*; bytes are a verified-by-hash second artifact.
2. **Query materialization = on-demand now, reactive later.** `query.materialize`
   snapshots via ordinary events; def-hash + source-cursor in the payloads make
   reactive refresh a v2 trigger, not a format change.
3. **Net secrets = redact on record.** Built-in sensitive-header list plus an
   app-declared list; `{"$secret": name}` reserved and later fulfilled by
   [cap-oauth-connections.md](cap-oauth-connections.md).
4. **Interop = MCP-shaped calls over the existing verb surface**, host-mediated
   (no in-QuickJS MCP client), replies recorded. **`common.receive` is required
   on every app** — validation rejects bundles without it, existing bundles get
   patched.
5. **Inbound email = interop delivery**, not new app surface: mail intake at
   the edge → `common.receive("email", …)` to the user-routed app (email v2).
6. **Web publish is Premium-gated** through the relay; the home host only ever
   dials out.
7. **Outbound messaging = `common.send`** (cap renamed from `email` to
   `common`; email is the first *channel*, grants are channel-scoped) —
   symmetric with `common.receive`. Common-verb set approved, later amended
   by decision 11: required = `receive` + `list` + `get`; scaffold generates
   defaults for all six.
8. **Person = local ed25519 keypair + attestations** — the keypair IS the
   identity; Premium/Google login, devices, emails attach as attestations,
   never define it.
9. **Org = shared home** — its own log/apps/data, members sync under
   person-signed role grants; **Premium hosting is a convenience offering,
   never a limitation**.
10. **Actor is a required field on every event** — `{kind, payload, actor}`,
    stamped by the engine; no backward compatibility — one-time migration
    stamps existing logs `user:local-owner`.
11. **Items are required-addressable** — `terrane://app/<id>/item/<itemId>`
    resolved live via `common.get`; the `items` interface (`list`+`get`) is
    mandatory on every app, empty item space valid.

## Agent-readiness follow-ups (small, not full cap plans)

Findings from the 2026-07-05 competitive audit (Sandstorm, Urbit, Val Town,
Tauri, Jazz/DXOS, ChatGPT Apps SDK, Cloudflare Workers) that are conventions
or extensions of existing plans, not new capabilities:

- **Bundle smoke tests** — builder validate gains an optional `tests.json`
  (verb + args + expected shape) run against the draft; agents self-verify
  what they build before commit. Extension of the existing builder cap.
- **Agent memory** — shell agents persist memory through their own app's kv
  scope; a convention documented in the agent cap, not new storage.
- **Model spend limits** — per-app rate/size caps on `model.*`/`local-model.*`
  calls, same decide-time pattern as `common.send` limits; fold into
  [cap-model-v2.md](cap-model-v2.md).
- **Durable multi-step workflows** — compose [cap-job-queue.md](cap-job-queue.md)
  + [cap-scheduler.md](cap-scheduler.md) (Cloudflare-Workflows shape); document
  the pattern in job-queue; only cap-ify if real usage demands it.
- **Per-item sharing granularity** — Sandstorm shares documents (grains);
  Terrane v1 shares apps ([cap-share-invite.md](cap-share-invite.md)). Known
  coarser granularity; per-document sharing is a crdt/document-level v2.
- **Agent permission scoping** — the agent cap already carries a security
  principal + grants (verify the deferred P1 from the top-bar-agents work is
  fully closed before agents get interop access).
- **MCP Apps compatibility** — the Apps-SDK widget protocol is standardizing;
  track it so Terrane apps can render inside ChatGPT/Claude hosts later.

## Suggested build order

0. **actor** — first, full stop: an engine/log-format change is cheapest
   before any other plan lands data, and every later cap inherits provenance
   for free.
1. **blob** → unblocks net v2, media, capture, tts, native-v2, webhook/stream
   offload, model-v2 images.
2. **net v2, query, time, document, telemetry, interop (+ item, same
   validation slice)** — mutually independent (interop is locked and
   high-leverage: email-receive, deep links, and the picker all ride it).
3. **connection** (→ **person** rides its keychain) → common.send,
   mcp-client; **scheduler** → job-queue → automation; **deep-links +
   history + browser** alongside.
4. **sync v2** → share-invite, presence, push, **org**; **web-publish** with
   the Premium relay.
5. **schema-migration** → app-update → publish; model-v2 alongside.
6. **compaction / backup-export** — whenever disk growth or portability starts
   to hurt; nothing user-visible blocks on them.

Every phase in every plan ends at the same gate:

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
