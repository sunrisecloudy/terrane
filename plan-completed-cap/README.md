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

## Index

### Foundation — data & core surface

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-blob.md](cap-blob.md) | `blob` | **Locked** | — |
| [cap-net-v2.md](cap-net-v2.md) | `net` (extended) | **Locked** | blob |
| [cap-query.md](cap-query.md) | `query` | **Locked** | kv, relational_db |
| [cap-document.md](cap-document.md) | `document` | Draft | — (names frozen by the live host's planned entry) |
| [cap-time.md](cap-time.md) | `time` | Draft | — |
| [cap-telemetry.md](cap-telemetry.md) | `telemetry` | Draft | js-runtime |

### Background work

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-scheduler.md](cap-scheduler.md) | `scheduler` | Draft | time, js-runtime, a long-running host |
| [cap-job-queue.md](cap-job-queue.md) | `job` | Draft | scheduler |

### Outbound & inbound integration

| Plan | Namespace | Status | Depends on |
| --- | --- | --- | --- |
| [cap-oauth-connections.md](cap-oauth-connections.md) | `connection` | Draft | net v2 (fulfils its `$secret` reservation), crypto primitives, OS keychain |
| [cap-webhook.md](cap-webhook.md) | `webhook` | Draft | blob, net v2 redaction rules |
| [cap-stream.md](cap-stream.md) | `stream` | Draft | net v2, blob; log growth ties to compaction |
| [cap-email.md](cap-email.md) | `email` | Draft | connection, blob |
| [cap-mcp-client.md](cap-mcp-client.md) | `mcp` | Draft | connection, blob |

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

## Suggested build order

1. **blob** → unblocks net v2, media, capture, tts, native-v2, webhook/stream
   offload, model-v2 images.
2. **net v2, query, time, document, telemetry** — mutually independent.
3. **connection** → email, mcp-client; **scheduler** → job-queue.
4. **sync v2** → share-invite, presence, push.
5. **schema-migration** → app-update → publish; model-v2 alongside.
6. **compaction / backup-export** — whenever disk growth or portability starts
   to hurt; nothing user-visible blocks on them.

Every phase in every plan ends at the same gate:

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
