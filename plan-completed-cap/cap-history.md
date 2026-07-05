# Capability: `history` — time travel over the event log

New crate `rust/crates/terrane-cap-history/`, namespace `history`. Terrane
already has what Sandstorm (grain backups), Jazz (git-like snapshots), and
Urbit (event-sourced state) advertise — a complete, ordered event log — but no
app or user can see it. This cap surfaces an app's own history: what changed,
when, and restore-to-point. It is the cheapest differentiator in the roadmap:
the data already exists; this is a read surface plus one command.

## Design principles

- **The log is never rewritten.** Restore emits ordinary compensating events
  (`kv.set` of the old values); replay-identity is untouched — history of the
  restore is itself history (the one rule survives intact).
- Reads are pure folds over a bounded slice — no new persistence. A per-app
  change index (reserved kv keys, like relational_db's pattern) makes
  key-level lookups cheap; it is a rebuildable projection.

## Command / query / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Query | `history.list` | `{app, filter?: kind\|key-prefix, before?, limit}` → paged `{seq, kind, summary}` (summaries via each cap's existing `describe()`) |
| Query | `history.key` | `{app, key, limit}` → change list for one kv key: `{seq, old, new}` |
| Query | `history.at` | `{app, key, seq}` → the key's value as of seq (index-accelerated) |
| Command | `history.revert` | `{app, to_seq, scope: key\|prefix\|app}` → computes current-vs-then diff at decide (pure fold), emits compensating `kv.set`/`kv.deleted` events + one `history.reverted {app, to_seq, scope, changed_count}` marker |
| Resource | `history.list/key/at(…)` | app-facing (an app can offer undo to its user) |
| Event | `history.reverted` | the marker fact; the compensations are ordinary kv events |

## User surface

- Shell (web + mac): per-app **History panel** — timeline from `history.list`,
  key drill-down, "restore to here" button wired to `history.revert` behind a
  confirm prompt.
- CLI: `terrane history <app> [--key k] [--at seq]`, `terrane revert <app>
  --to <seq> [--scope …]` (dry-run prints the diff by default; `--yes`
  applies).

## Scope & interactions

- **v1 scope: `kv` (and therefore `relational_db`, which lives on kv reserved
  keys — revert by prefix covers tables).** CRDT documents keep their own
  history inside Loro (checkout/versions) — surfacing that is a v2 that rides
  the same panel; `document` history falls out of its events once that cap
  lands. Blobs: reverting metadata re-points names at old hashes — bytes are
  content-addressed so old versions still exist unless GC'd; the revert
  decide refuses (typed error) if a needed hash's refcount already dropped
  and GC ran — cross-link [cap-blob.md](cap-blob.md).
- **Compaction** ([cap-compaction.md](cap-compaction.md)): history reads
  beyond the snapshot horizon consult the archived log segment; if archives
  were deleted, `history.list` states its horizon honestly (`from_seq`).
  Revert only needs the *values at to_seq*, which the horizon may still cover
  via the snapshot — the decide errors clearly when it cannot.

## Security & permissions

An app may read only **its own** history (`history` resource grant); the
shell/operator sees any app's. Revert is destructive-adjacent: always behind
an explicit confirm (shell prompt / CLI `--yes`), and recorded with the marker
event so reverts are auditable and themselves revertable.

## Limits

`list` page ≤ 500; `at`/`key` bounded by the index; revert scope `app` capped
at 10 000 changed keys (typed error above — split by prefix).

## Implementation plan

1. **Change index projection:** per-app `key → [seq]` reserved-kv index built
   in fold (rebuildable; versioned like other projections).
2. **Crate:** queries over log slice + index; `history.revert` decide (pure
   diff computation) + fold; doc.
3. **Host:** log-slice read access for queries (the host owns `log.bin`);
   CLI subcommands with dry-run default.
4. **Shell:** History panel (web first, mac parity), confirm flow.
5. **`APP_API.md`:** `ctx.resource.history.*` + an undo-button example.
6. **Tests:** engine (at/list/key correctness against a scripted event
   sequence, revert compensations, revert-of-revert, replay identity with the
   index projection); e2e (CLI dry-run vs apply, horizon behavior with a
   truncated log fixture).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

CRDT/Loro version surfacing (v2), cross-app timelines, branching/forking
state (restore is linear), automatic periodic snapshots-as-bookmarks, diff
UIs beyond key old/new.

## Decisions to confirm

- **Index granularity** — recommend per-key seq index (fast `history.key`,
  modest storage) — alternative: no index, fold-on-demand (zero storage, slow
  on big logs).
- **Revert semantics for keys created after `to_seq`** — recommend delete
  them within scope (true point-in-time) — alternative: keep unknown keys
  (safer, but not really "restore").
