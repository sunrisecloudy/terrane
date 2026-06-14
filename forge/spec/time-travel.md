# File-level time travel (DL-20) — per-record change feed + non-destructive restore

> Spec. The behavioral contract is `forge/fixtures/time-travel/` (the manifest +
> case vectors) plus the unit tests in `forge-storage` (`src/time_travel.rs`,
> `src/compaction.rs`).

prd-merged/02-data-layer-prd.md **DL-20**: *"File-level time travel (v1, P-06):
per-doc history view; restore creates a new version, never destructive rollback;
per-record change feed (who/when/what) retained 90 days (configurable) powers undo
and audit."*

Time travel is a **read + append** view over the substrate that already exists:
every record mutation appends one immutable `crdt_chunks` row (the CRDT op — the
DL-6 source of truth) AND one `oplog` row carrying provenance, in a deterministic
`(lamport, op_id)` total order matching write order (DL-4). DL-20 adds no new
storage; it *derives* a per-record change feed from that history and adds a
non-destructive restore that writes a NEW op equal to a prior state.

## 1. Version

A record's **version** is the per-doc **chunk frontier**: the zero-padded sequence
number in a chunk id (`chunk-0007 → 7`, a compact snapshot `compact-0003 → 3`). This
is the same derivation the oplog lamport and DL-19 compaction use, so a feed entry's
`version` equals the oplog `lamport` of the op that produced it. It is a **logical
clock** — never a wall clock — so every time-travel read and the restore replay
deterministically (the SC-12 / audit-log determinism lesson).

To reconstruct a record's state **as of** version `v`, replay the chunks with
frontier `≤ v` into a fresh `RecordsDoc` (DL-6 rebuild-by-replay, bounded by the
frontier) and read the record. This is order/duplication independent (Loro dedupes
by version), so the result is stable.

## 2. Change feed shape (WHO / WHEN / WHAT)

`Store::record_history(collection, id)` returns the ordered change feed: one
`HistoryEntry` per version that touched the record, oldest-to-newest by `version`.

```jsonc
{
  "version": 2,                 // the chunk frontier this entry advanced the record TO
  "actor": "local",             // WHO — oplog actor_id (a synced write carries the original author)
  "source": null,               // WHO (provenance) — remote-import origin when forwarded; null if local
  "logical_at": 2,              // WHEN — the supplied LOGICAL timestamp (envelope.updated_at; for a delete, the delete's own logical_at)
  "kind": "record.patch",       // WHAT — the oplog op kind
  "state": { /* RecordEnvelope as of v2 */ }   // the record's full state here; null when tombstoned
}
```

- **WHO** — `actor` is the oplog `actor_id`. A locally-authored write is `"local"`;
  a write that arrived via sync carries the original author's peer id (review
  092/101). `source` is the remote-import provenance (the origin a relay forwarded
  from), `null` for a local write.
- **WHEN** — `logical_at` is the record envelope's `updated_at`, i.e. the
  externally-supplied `logical_at` of the mutation. It is a **logical** timestamp;
  the replayable path never reads a wall clock. A delete leaves no surviving
  envelope, so its WHEN is recovered from the delete's own `logical_at`, carried on
  the oplog row (the `mutation_at` field) — the tombstoned version stays dated so the
  monotone restore clock (§3) counts it (DL-20 review 169).
- **WHAT** — `kind` is the oplog op kind: `record.insert`, `record.update`,
  `record.patch`, `record.delete`, or `schema.migration` (a migration that rewrote
  the record IS a change to it and appears in the feed). `state` is the record's
  full envelope reconstructed at that version, or `null` when the record was
  tombstoned (a `record.delete`, or any version where the record is absent).

Only versions that **touched the named record** appear (the oplog row's
`record_ids` names it), so a sibling record's edits never leak into another record's
feed. A `history.compact` row (a DL-19 compaction) names no record and produces no
feed entry — compaction is not a change to the record.

`Store::record_state_at(collection, id, version)` is the point read behind the feed:
the record's envelope as of one version (or `null`). `Store::latest_version(collection)`
returns the current frontier (the "current version" handle).

## 3. Non-destructive restore

`Store::restore_record(collection, id, to_version, restored_logical_at, indexes)`
brings a record back to its state **as of** `to_version` by appending a **NEW**
version — never a destructive rollback, never a rewrite of prior history.

1. Reconstruct the record at `to_version` (§1).
2. Write that state through the **same DL-4 mutation path** the live spine uses
   (`apply_mutation_crdt`): a fresh `record.insert` carrying the prior envelope's
   display fields. An `Insert` is a full (re)create even over a tombstone (DL-21
   reinsert), so it works whether the record is currently live or deleted.
3. Return the **new version** (the chunk frontier the restore appended) as the
   audit/undo handle.

Contract:

- **Append-only, never destructive.** `crdt_chunks` and `oplog` are append-only;
  the restore writes ONE new chunk + one new oplog row. Every prior chunk/oplog row
  — *including the versions written after `to_version`* — remains intact and stays
  reconstructable. The change feed grows by one entry; its earlier entries are
  byte-identical to before.
- **Restoring a deleted version re-creates the record.** If `to_version` is a
  tombstone (or pre-history), the target state is "absent". Restoring it onto a live
  record appends a `record.delete` (the record becomes absent as a new version);
  restoring a non-tombstone version onto a deleted record reinserts it.
- **`restored_logical_at`** is the LOGICAL timestamp stamped on the new version (a
  logical clock, no wall clock), so the restore replays deterministically. Reusing
  the spine's `logical_at` keeps `updated_at` monotone.
- **Replay-safe.** The restore op is in the CRDT source of truth, so a DL-6
  `rebuild_projection` reproduces the restored state exactly.

M0a scope: restore re-creates the record's **display fields** (`fields`). Carrying
`unknown_fields`/`extensions` verbatim through a restore is a v1+ extension.

## 4. Retention

The change feed (the oplog rows powering undo/audit) is retained for a
**configurable window**, default ~90 days. M0a models the window as a count of
**logical versions** (the per-doc chunk frontier IS the logical clock of the chunk
stream — a logical clock, never a wall clock, so retention stays
replay-deterministic).

`RetentionPolicy { window }` (default `DEFAULT_RETENTION_WINDOW = 90`) is attached
to `CompactionOptions` (`CompactionOptions::with_retention`). When set, the
most-recent `window` logical versions of change-feed/oplog history are **protected
from pruning**, even when the DL-19 safe horizon (or `allow_peer_reset`) would
otherwise fold those chunks into a compact snapshot:

- given the current frontier `now_version`, the **protected floor** is
  `now_version - window + 1` (saturating); a chunk/oplog row whose frontier is `≥`
  the floor keeps its standalone oplog row.
- DL-19 and DL-20 floors **both apply** — the compaction boundary is the lower of
  the safe horizon and the retention cap.
- entries **beyond** the window may be pruned (subject to the safe horizon), so
  history past the configured retention can be compacted away.

Retention only governs the **standalone change-feed entries**: a folded chunk's
record *state* is still represented by the compact snapshot's frontier (the
projection is unchanged by compaction — the DL-19 invariant), but its individual
who/when/what oplog row past the window is no longer retained.

## 5. RBAC

History reads (`record_history`, `record_state_at`) require `db.read` on the
collection; restore (`restore_record`) requires `db.write`. The grant is scoped from
the **trusted context**, never the request payload (review 048/050). These
`forge-storage` methods are the substrate read/write; the gate is enforced by the
caller (`forge-core`).

## 6. Determinism summary

- Version = chunk frontier = oplog lamport (logical, no wall clock).
- History read = pure replay of chunks ≤ version → byte-stable across calls.
- Restore = append a new op on the DL-4 path → replay-safe (survives DL-6 rebuild).
- Retention window = a logical-version count → compaction stays deterministic.

The whole surface keeps `forge demo` REPLAY IDENTICAL.
