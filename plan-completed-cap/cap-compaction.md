# Engine: event-log compaction & snapshots

**Not a capability crate.** This is engine work in `terrane-core` plus host
commands in `terrane-host` — the filename keeps the `cap-` prefix only for
series consistency. There is no new namespace, command grammar for apps, or
`ctx.resource` surface: apps never see compaction. What it buys is purely
operational — `$TERRANE_HOME/log.bin` grows forever today, and every
`Core::open_with` folds the whole history. Nothing user-visible unlocks, so
this is **low priority**: schedule it when a real home's open time or disk
footprint hurts, and after [cap-backup-export.md](cap-backup-export.md)
(backups are the safety net for a truncating operation).

## Recommended decision

**Capability-owned snapshot sections, sidecar file, archived old log.** A
snapshot is the folded `State` at log position `N`, serialized as name-tagged
sections — each capability serializes its own slice, exactly as events are
name-tagged `{kind, payload}` — written to `$TERRANE_HOME/snapshot.bin`;
`log.bin` is then atomically rewritten to contain only events after `N`, and
the full pre-compaction log is preserved as `log.bin.archive`. Serializing the
aggregate `State` centrally is rejected: it would put every capability's
private state layout into one central format (the exact central-enum shape the
architecture bans) — instead the `Capability` trait grows optional
`snapshot() -> Option<Vec<u8>>` / `restore(&[u8])` hooks, and `crdt`
implements its own using **Loro's compact snapshot export**, which preserves
the per-doc history that version-vector sync depends on.

## File format

```text
snapshot.bin :=
  header  { format_version: u32 = 1, seq: u64 /* events folded */,
            log_head_hash: [u8; 32] /* sha256 of the archived log bytes */ }
  section*{ namespace: String, payload: Vec<u8> }        // borsh, like events
```

- Sections exist only for capabilities that returned `Some` from `snapshot()`;
  a capability with empty state writes nothing.
- Restore of an unknown namespace is a typed `Storage` error (a home touched
  by a newer terrane), never a silent skip.
- `log.bin` framing is unchanged (u32-LE length + borsh `EventRecord`);
  `EventRecord` has no seq field, so `seq` is the record *count* the snapshot
  covers — position in the log is the only ordering there is today, and this
  keeps it that way.

## Open & replay-identity, redefined

`Core::open_with` becomes: load `snapshot.bin` if present (restore each
section into a default `State`), then fold `log.bin` on top; then, as today,
`sync_full_storage` / `sync_reserved_projection` rebuild the physical
projections — projections need no compaction awareness at all.

The determinism contract is restated, and tested, as:

```text
restore(snapshot(N)) + fold(events N..)  ≡  fold(all events)
```

The proving test compacts a populated core, re-opens it, and `assert_eq!`s the
two `State`s (`State` is `PartialEq`, not `Eq`, because crdt holds `f64` —
comparison, not hashing, is the check). `Core::replay_matches()` is updated to
fold snapshot+tail, and a second test verifies the crdt section round-trips
Loro history: post-compaction `crdt_export_from_vv` answers a stale peer
version vector identically to the uncompacted home.

## Interactions (the actual design work)

- **Sync.** Both sync paths (`terrane sync <app> --from <home>` and the TCP
  peer sync) are **crdt-delta based**: they exchange Loro version vectors and
  ops exported from *folded state*, never raw terrane events. So compaction
  does not break existing sync — provided the crdt snapshot section keeps full
  Loro history (Loro's default snapshot does; its shallow/gc snapshot would
  not, and is explicitly not used). For a future event-level sync, the lever
  is a **retain window**: `terrane compact --retain <n>` keeps the last `n`
  events in the live log (default 0 — no such consumer exists today), and
  `log.bin.archive` remains the escape hatch for a peer that is further
  behind.
- **Blob CAS GC.** [cap-blob.md](cap-blob.md) rules `terrane blob gc` "never
  automatic — a future event-log compaction could otherwise race it".
  Resolved here: both operations open the core, so both hold the exclusive
  home file lock (`terrane_core::filelock`) and serialize by construction;
  compaction never touches `blobs.sqlite3`; and gc's input (refcounts in
  folded state) is replay-identical before and after compaction, so ordering
  between the two is immaterial. Both stay manual commands.
- **Backup/restore.** Archives include `snapshot.bin` + `log.bin` as a pair
  (the header's `log_head_hash`/`seq` bind them); restore then integrity-check
  work unchanged — see [cap-backup-export.md](cap-backup-export.md).
- **Migrations** ([cap-schema-migration.md](cap-schema-migration.md)) record
  ordinary events, so they compact like anything else; the folded
  `migration.applied` fact survives in its capability's section.

## Host surface & safety

- `terrane compact` — prints projected sizes and asks nothing else; steps:
  acquire home lock (via open) → write `snapshot.bin.tmp` → fsync → rename →
  rewrite `log.bin.tmp` with the tail → rename → move full log to
  `log.bin.archive`. Every step is crash-safe: a `.tmp` is ignored on open,
  and until the final rename the old log is authoritative.
- `terrane compact --verify` — run the equivalence check before touching disk
  (fold archive fully vs snapshot+tail) and refuse on mismatch.
- `log.bin.archive` is kept until the user deletes it or runs
  `terrane compact --prune-archive`; compaction is the one operation that can
  destroy history, so v1 never deletes it silently.

## Implementation plan

1. **Trait hooks:** optional `snapshot`/`restore` on `Capability`
   (`terrane-cap-interface`), default `None`/error; implement for the stateful
   built-ins (borsh of their state slice) and for `crdt` via Loro snapshot
   bytes.
2. **Format + IO:** `terrane-core/src/snapshot.rs` — write/read
   `snapshot.bin`, header hashing, section framing; typed errors throughout.
3. **Open path:** `Core::open_with` loads snapshot-then-tail;
   `replay_matches` updated; `read_log` untouched.
4. **Host command:** `terrane compact` (+ `--verify`, `--retain`,
   `--prune-archive`) in `terrane-host/src/cli.rs` with the crash-safe
   rename dance.
5. **Tests:** engine `terrane-core/tests/cap/compaction.rs` — the
   equivalence test above, crdt vv-sync-after-compaction, unknown-section
   error, crash-simulation (leftover `.tmp` files); e2e
   `terrane-host/tests/cap/compaction.rs` — compact a real home, re-open,
   run an app, `terrane blob gc` ordering smoke test once blob lands. All
   default-run.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Automatic/scheduled compaction, shallow (history-dropping) crdt snapshots,
incremental/delta snapshots, snapshot exchange over the wire (retain window +
archive cover it), compacting `blobs.sqlite3` (that is blob gc's job), any
app-visible surface.

## Decisions to confirm

- **Snapshot ownership** — per-capability `snapshot`/`restore` trait hooks,
  name-tagged sections — alternative: one central borsh of `State` (simpler
  now, but centralizes every capability's private layout and breaks the
  no-central-format rule).
- **Old-log retention** — keep `log.bin.archive` until explicitly pruned —
  alternative: delete on success (smaller disk, no second chance).
- **Sync stance** — rely on Loro history inside the crdt section, retain
  window default 0 — alternative: mandatory retain window or peer snapshot
  exchange (build it when an event-level sync consumer exists).
- **Priority** — defer until open-time/disk pain is real; land backup/export
  first — alternative: land now as pure hygiene.
