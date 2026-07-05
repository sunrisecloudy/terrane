# Host: `backup` — home backup, restore, and per-app export

**Host-level work** in `terrane-host` + the CLI adapter — no new capability
crate, no new namespace, no app surface. A Terrane home
(`$TERRANE_HOME`: `log.bin` event log, `terrane.db` kv projection,
`log.bin.lock`, installed `apps/<id>/` bundles, and — once their plans land —
`blobs.sqlite3` and `snapshot.bin`) is the user's entire world, and today the
only "backup" is copying a directory and hoping no writer was mid-append.
This adds a consistent, versioned, verifiable archive format plus a selective
per-app export.

## Recommended decision

**`tar.zst` archive with a `manifest.json`, log-authoritative contents.** The
archive contains exactly what cannot be rebuilt: the event log (+ snapshot
pair), the blob CAS, the app bundles, and a manifest that binds them with
hashes. Everything derivable is excluded — `terrane.db` and the other kv/auth
projections are rebuilt by `Core::open_with` via `sync_full_storage` on first
open, and `log.bin.lock`/sockets are live-state, never archived. Consistency
comes from the existing exclusive home lock (`terrane_core::filelock`):
backup acquires it for the duration of the copy, so no writer can interleave.

## Archive format (v1)

`terrane backup create <path.tzst>` writes:

```text
manifest.json      { "formatVersion": 1, "createdAtMs": …, "terraneVersion": "…",
                     "logRecords": <count>, "peer": "<hex replica id>",
                     "files": [ { "path": "log.bin", "sha256": "…", "bytes": … }, … ] }
log.bin            # the event log — the source of truth
snapshot.bin       # if present (compacted home, see cap-compaction.md)
blobs.sqlite3      # if present (cap-blob.md CAS) — copied via SQLite's
                   # backup API / `VACUUM INTO`, not a raw file copy
apps/…             # installed bundles (app.source paths point here)
```

- `manifest.json` is first in the tar so `terrane backup info <path>` can
  answer from a stream without unpacking.
- Hashing is SHA-256 per file (the CAS convention); the manifest itself is the
  integrity root. There is no whole-`State` hash: `State` is `PartialEq` only
  (crdt holds `f64`), so state-level verification is *comparison by refold*,
  not hashing — see integrity below.

## Restore

`terrane backup restore <path.tzst> --into <home>`:

1. Refuse unless `<home>` is absent or empty — restore never merges.
2. Unpack; verify every `files[]` sha256; typed error on any mismatch.
3. Open the core read-only style (`open_at_home`): this folds
   snapshot+log — a fold error means a corrupt/incompatible archive — and
   rebuilds all projections as a side effect.
4. Integrity check = `logRecords` count matches, every hash matches, and
   `Core::replay_matches()` holds on the opened core. Print the folded
   summary (apps, record count, peer id) so the user sees what came back.

### Replica identity — the sharp edge

The replica id is minted once by `replica.init` and recorded as the
`replica.initialized` **event in the log**, so a restored home has the same
PeerID *by construction* — which is exactly right for a true restore (dead
disk, new machine, old home retired) and exactly wrong for a copy: the same
home restored twice and both written to means two replicas authoring crdt ops
under one `(peer, counter)` space — silent write loss on merge.
**Restore keeps identity; cloning re-mints it:**
`terrane backup restore --clone` additionally dispatches a new
`replica.rotate` command (small addition to `terrane-cap-replica`: mints a
fresh edge-random peer id and records a new `replica.initialized`, whose fold
replaces `ReplicaState::peer`; existing crdt ops keep their original author,
new ops author under the new peer). The restore output always states which
mode ran, and plain `restore` warns about the duplicate-identity rule.

## Selective per-app export

`terrane export <app> <path.tzst>` — the portable sibling of
`terrane sync <app> --from <home>`. Note the existing sync is **crdt-only**
(it exports Loro deltas and dispatches `crdt.merge`; kv/relational data never
travels), so export must be broader: the app's full event history plus its
referenced blobs and bundle.

- **Event slicing:** `EventRecord` is `{kind, payload}` with capability-private
  payloads, so the host cannot filter by app today. Add an optional
  `Capability::app_of(&EventRecord) -> Option<AppId>` hook (the same shape as
  the existing `decode_app_removed` helper); events whose owner returns the
  target app — plus the app's own `app.*` lifecycle events — are copied **in
  log order** into the archive's `log.bin`. Platform events (auth grants for
  the app) are excluded in v1: grants are re-approved on the destination, the
  honest behaviour for a cross-machine move.
- **Blob pass:** collect the hashes referenced by the app's folded blob state,
  copy those CAS rows into an archive-local `blobs.sqlite3`
  ([cap-blob.md](cap-blob.md) sync pattern).
- **Import:** `terrane import <path.tzst>` appends the sliced events to the
  local log (refusing if the app id already exists — no merge semantics; that
  is crdt sync's job) and copies blobs `INSERT OR IGNORE`.

## Limits & safety

- Backup of a home holding `log.bin.lock` from a live process fails fast with
  the existing lock error — never a torn copy.
- Archive size is unbounded in principle; `backup create` prints per-file
  sizes and the CLI streams (tar → zstd) rather than staging in memory.
- `zstd` level 3 default (`--level` flag); format is tar + zstd, both boring
  and universal — the manifest's `formatVersion` is the upgrade lever.

## Implementation plan

1. **Module:** `terrane-host/src/backup.rs` — manifest types (serde),
   archive writer/reader (workspace deps: `tar`, `zstd`), lock-held file
   collection, sha256 (reuse the workspace `sha2`).
2. **CLI:** `terrane backup create|restore|info` in `cli.rs`, `--clone` and
   `--into` flags; help text drift-guarded like existing commands.
3. **Replica:** `replica.rotate` command + event fold in
   `terrane-cap-replica` (edge-minted id, same `initialized_event` pattern).
4. **Export/import:** `Capability::app_of` hook (default `None`) implemented
   for app-scoped capabilities (kv, crdt, net, relational_db, …);
   `terrane export|import` using it + the blob pass.
5. **Tests:** e2e `terrane-host/tests/cap/backup.rs` — create→restore→
   `replay_matches` + state equality against the source; tamper a byte,
   restore refuses; restore-into-nonempty refuses; `--clone` yields a
   different peer and two clones cross-sync via crdt without loss; export→
   import round-trips one app's kv/crdt data and refuses on existing id.
   All default-run (no network).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Scheduled/automatic backups, incremental backups (compaction +
[cap-compaction.md](cap-compaction.md) archives are the growth answer),
encryption at rest (archive inherits filesystem trust; a `crypto`-backed
sealed archive is a follow-up), restore-merge into a non-empty home, exporting
auth grants, cloud targets.

## Decisions to confirm

- **Archive format** — tar + zstd + `manifest.json` with per-file SHA-256 —
  alternative: a single SQLite container (self-describing but reinvents tar),
  or plain zip (worse compression, no streaming append).
- **Integrity definition** — file hashes + full refold + `replay_matches`,
  no state hash — alternative: define a canonical `State` digest (blocked
  today by crdt `f64`/`PartialEq`; would need per-capability hash hooks).
- **Replica identity on restore** — restore keeps the PeerID; `--clone`
  re-mints via new `replica.rotate` — alternative: always re-mint (breaks the
  true-restore case: the restored home would look like a foreign replica to
  peers that knew it).
- **Per-app slicing mechanism** — `Capability::app_of` hook over the raw log —
  alternative: re-emit the app's folded state as fresh events (no hook, but
  loses history and invents a second serialization of every capability's
  data).
- **Grant portability** — exclude auth grants from export; re-approve on
  import — alternative: carry grants and trust the source home's approvals.
