---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/spec/live-queries.md, forge/fixtures/live-queries/*.json, forge/fixtures/live-queries/manifest.json
---

# T035 — Live queries / db.watch vectors (DL-16)

The audit ranks live queries as the reactive complement to the event loop. db.watch:
register SQLite update-hook-backed watchers, track a dirty set, notify applet
callbacks async (spec p95 < 30ms local). I want spec + vectors before the Rust wiring.

## Deliverables
1. `forge/spec/live-queries.md` — derive from prd-merged/02 (DL-16), the storage write
   path (forge/crates/storage/src/crdt_write.rs emits the signal points) and ctx.db.query
   (DL-15). Define the watch registration, the change notification shape (which collection
   + changed record ids + a monotonic version), dirty-set semantics, dedup/coalescing,
   unwatch, and the determinism/replay note (watch notifications must be recorded so a
   session replays identically).
2. `forge/fixtures/live-queries/<case>.json` + manifest. Each: an initial dataset + a
   registered watch (collection/query), a mutation, and the expected notification (or none).

## Coverage (~10)
insert into watched collection -> notify with the new id; update -> notify; delete ->
notify; a mutation to a NON-watched collection -> no notify; a watch with a query filter
where the change doesn't match -> no notify; two watchers on the same collection both
notified; unwatch stops notifications; a batch/transact mutation -> one coalesced notify
with all changed ids; notification carries a monotonic version; re-running the recorded
session replays identical notifications.

In `## Result`, flag the determinism decision (notifications recorded in the run record so
replay is byte-identical) and the coalescing rule for a multi-record transaction.

## Result
Delivered `forge/spec/live-queries.md` and `forge/fixtures/live-queries/` with 10 semantic JSON vectors plus `manifest.json`.

Contract decisions encoded:

- Watch notifications are recorded in the run/session record as `db.watch.notification` entries so replay does not depend on live SQLite hooks and remains byte-identical.
- A committed multi-record transaction produces one dirty set and at most one notification per affected watch; `record_ids` are sorted/deduplicated and the notification is marked `coalesced: true`.
- Notification versions are workspace-local monotonic versions assigned to committed write transactions; notifications from the same transaction share one version, later transactions get greater versions.
- Filtered watches notify only when the query result may change; a dirty row outside the result both before and after the mutation produces no notification.
