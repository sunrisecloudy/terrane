# Capability: `blob` — binary storage

New crate `rust/crates/terrane-cap-blob/`, registered in `default_registry`.
Gives apps binary storage (images, attachments, files) that today is impossible:
`kv` is string-only.

## Locked decision

**Content-addressed sidecar.** Events record only metadata
(`{name, hash, size, mime}`); bytes live in a per-home SQLite database in a blob
table keyed by content hash (CAS). Replay rebuilds all metadata state from the
log and treats the CAS as a verified-by-hash artifact — it never needs the
network or re-hashing to fold, and a missing/corrupt blob is a typed read-time
error, not a replay failure.

## Storage layout

- File: `$TERRANE_HOME/blobs.sqlite3` (sidecar, sibling of the event log).
  Opened by the host edge, never by the core — the core sees only events.
- Schema (v1, `PRAGMA user_version = 1`):

```sql
CREATE TABLE IF NOT EXISTS blobs (
  hash  TEXT PRIMARY KEY,   -- lowercase hex SHA-256 of bytes
  size  INTEGER NOT NULL,
  bytes BLOB NOT NULL
) STRICT;
```

- Hash: **SHA-256, lowercase hex** (64 chars). Boring, universal, and stable
  across replicas — the hash is the sync/identity key, so no "faster hash later"
  swap; the algorithm is part of the wire format.
- Dedup is free: same bytes from two apps → one row. Rows are refcounted
  logically via folded state (see GC).

## Capability surface

Namespace `blob`. App-scoped named blobs: an app addresses blobs by `name`
(any non-empty string, `/`-separated paths encouraged); the fold keeps
`app → name → BlobMeta { hash, size, mime }`.

### Commands

| Command | Args | Decision |
| --- | --- | --- |
| `blob.put` | `app, name, mime, bytes_base64` | Validate (size cap, base64, name), compute hash + size **in decide** (pure), return `Decision::Effect(Effect::BlobStore { app, name, mime, hash, bytes })`. Runner inserts bytes into CAS (idempotent `INSERT OR IGNORE`), emits `blob.stored`. |
| `blob.rm` | `app, name` | Pure: emit `blob.removed` if the name exists. |
| `blob.link` | `app, name, hash, size, mime` | Pure: name an *existing* CAS hash (used by sync and by net v2 body offload); errors if state has never seen the hash is **not** checked (CAS presence is an edge concern), emit `blob.stored`. |

Computing the hash in `decide` keeps the event deterministic and lets the
runner stay dumb: write bytes, return the capability-owned event (same
`fetched_event`-style constructor pattern as `net`).

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `blob.stored` | `{ app, name, hash, size, mime }` | upsert `app→name→meta`; bump logical refcount for `hash` |
| `blob.removed` | `{ app, name, hash }` | drop name; decrement refcount |
| (reacts) `app.removed` | — | drop all names for app; decrement each hash refcount |

### Resource methods (JS: `ctx.resource.blob`)

| Method | Semantics |
| --- | --- |
| `put(name, base64, mime)` | routes to `blob.put`; returns `hash` |
| `get(name)` | base64 bytes — resource call; edge reads CAS by hash from folded meta, **verifies SHA-256 on read**, typed `BlobMissing`/`BlobCorrupt` errors |
| `stat(name)` | JSON `{hash, size, mime}` (pure state read) |
| `list(prefix)` | JSON array of `{name, hash, size, mime}` (pure state read) |
| `rm(name)` | routes to `blob.rm` |

Grant resource: `blob` namespace-v1 with `call` methods, described as
"binary blob storage" — flows through the existing auth permission prompts
unchanged.

### Host/UI surface

- `window.terrane.blobUrl(name)` in the web/mac shells resolves to a
  host-served route `GET /app/<app>/blob/<name>` (permission-checked, correct
  `Content-Type` from meta, `ETag: hash`) so `<img src=...>` works without
  base64 round-trips through JS.
- CLI: `terrane blob put|get|ls|rm <app> ...` (thin adapter, host-side file IO
  for put/get paths — the file read is the host's job, the core only ever sees
  base64/bytes in the request).

## Limits (documented in `doc.rs`, enforced in decide)

- Max blob size: **64 MiB** (base64 transit ⇒ ~85 MiB request; QuickJS string
  is fine at this size, chunked upload is a v2 if ever needed).
- Max name length 512; names are exact strings, no normalization.
- Per-app blob count soft cap 10 000 (typed error, overridable via home config
  later if a real need appears).

## Replay & sync

- **Replay:** folding the log rebuilds the full name→meta map with zero CAS
  access. Replay-identity tests never touch SQLite.
- **Integrity:** reads verify hash; `terrane blob verify` (host command) scans
  state's live hashes against CAS and reports missing/corrupt rows.
- **Sync:** `terrane sync <app> --from <home>` grows a blob pass *after* the
  event pass: collect hashes referenced by the synced app's folded state,
  copy rows missing from the local CAS (`ATTACH` the source DB, `INSERT OR
  IGNORE ... SELECT`). Events-before-blobs ordering means a crashed sync leaves
  dangling meta (read → typed `BlobMissing`), never orphan bytes semantics
  breakage; re-running sync heals it.

## GC

Fold maintains `hash → refcount` in `BlobState`. Bytes deletion is an edge
concern: `terrane blob gc` (host command, dry-run by default) deletes CAS rows
whose hash has refcount 0 in current state. **Never automatic in v1** — a
future event-log compaction could otherwise race it.

## Implementation plan

1. **Interface:** add `Effect::BlobStore { app, name, mime, hash, bytes }` to
   `terrane-cap-interface::abi` (bytes as `Vec<u8>`; the effect is transient —
   the *event* carries no bytes).
2. **Crate:** `terrane-cap-blob` — `lib.rs` (Capability impl: manifest, decide,
   fold, resource reads, describe), `doc.rs`, `stored_event()` constructor.
   `sha2` dep (workspace). No rusqlite here — the crate is pure.
3. **CAS module:** `terrane-host/src/blob_store.rs` — open/create
   `blobs.sqlite3`, `insert_if_absent(hash, bytes)`, `read(hash)`,
   `verify(hash)`, gc query. Wire `Effect::BlobStore` into `EdgeRunner::run`.
   Resource `get` read path wired through the existing `LiveHost` hook (same
   pattern sysinfo uses for live reads).
4. **Register** in `default_registry`; scaffold/app-recipe docs gain `blob` in
   the resources list; `APP_API.md` documents `ctx.resource.blob` and
   `window.terrane.blobUrl`.
5. **Hosts:** web + mac shell blob route with permission check; CLI `blob`
   subcommands.
6. **Sync:** blob pass in `terrane-host/src/sync.rs`.
7. **Tests:** engine tests `terrane-core/tests/cap/blob.rs` (decide/fold/replay
   identity, size caps, app.removed); e2e `terrane-host/tests/cap/blob.rs`
   (real CAS round-trip, corrupt-byte detection, gc dry-run, sync blob pass);
   all default-run (no network).

Gate after each numbered step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.
