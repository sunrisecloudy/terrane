# Live Queries / `db.watch`

Source of record: `prd-merged/02-data-layer-prd.md` DL-16, `prd-merged/01-core-runtime-prd.md` CR-6/CR-8, and the query surface in `forge/spec/query-dsl.md`.

Live queries are the reactive form of the same typed query plan used by `ctx.db.from(...).all()`. They let a long-lived applet subscribe to projection changes without polling. Raw SQL is still never exposed to applets.

## Registration

Applet API shape:

```ts
const subscription = await ctx.db.watch(
  ctx.db.from("tasks").where(f => f.done.eq(false)),
  event => {
    // applet callback; usually triggers ctx.ui.render(nextTree)
  }
);

await subscription.unwatch();
```

Host-call shape:

```json
{
  "method": "db.watch",
  "args": {
    "watch_id": "watch:tasks-open",
    "query": { "from": "tasks", "where": ["done", "=", false] }
  }
}
```

Rules:

- Registration requires the same `db.read` grant as `all()` for the watched collection.
- The query AST is the canonical validated query plan from `forge/spec/query-dsl.md`.
- `watch_id` is runtime-assigned and stable until `db.unwatch`.
- `db.unwatch(watch_id)` is idempotent. After it commits, the watch receives no further notifications.
- A watch observes the rebuildable `records` projection after each committed mutation transaction.

## Dirty Set

The storage write path builds a dirty set per committed mutation transaction:

```json
{
  "version": 12,
  "collections": {
    "tasks": ["tasks/1", "tasks/2"]
  }
}
```

Rules:

- The dirty set is produced after the SQLite transaction commits and after the projection/index rebuild succeeds.
- Record ids are deduplicated and sorted by `entity_id` for deterministic notification bytes.
- `insert`, `update`, `patch`, and `delete` all dirty their target record id.
- `transact([...])` produces one dirty set and at most one notification per affected watch.
- Mutations that roll back produce no dirty set and no notifications.

## Notification Shape

Canonical callback payload:

```json
{
  "type": "db.watch.notification",
  "watch_id": "watch:tasks-open",
  "version": 12,
  "collection": "tasks",
  "record_ids": ["tasks/1"],
  "reason": "changed",
  "result_ids": ["tasks/1", "tasks/3"],
  "coalesced": false
}
```

Fields:

- `version`: a workspace-local monotonic watch version assigned to committed write transactions. Notifications from the same transaction share the same version. Later notifications must have a greater version.
- `collection`: the watched collection.
- `record_ids`: dirty ids in that collection that caused this notification, sorted and deduped.
- `reason`: `insert`, `update`, `delete`, `changed`, or `mixed`. A transaction touching multiple operation kinds reports `mixed`.
- `result_ids`: the current matching query result ids after the transaction, in query order. This lets applets re-render without issuing an immediate follow-up `all()` when the runner supports it.
- `coalesced`: `true` when multiple dirty ids were folded into one notification.

## Filter Semantics

For a filtered watch, a dirty record notifies only when the query result may have changed:

- record enters the result set: notify;
- record leaves the result set: notify;
- record stays in the result set but selected fields changed: notify;
- record was outside the result before and after the transaction: no notify.

The runtime may conservatively notify if it cannot prove the result is unchanged, but fixtures pin the preferred deterministic behavior for simple single-collection filters.

Deletes are evaluated as a result-set change. Tombstoned records are hidden unless the query explicitly sets `includeDeleted`.

## Replay

Watch notifications are part of the event loop, so they must be recorded in the run/session record with the same status as UI events and host-call responses:

```json
{
  "method": "db.watch.notification",
  "args": {
    "watch_id": "watch:tasks-open",
    "version": 12,
    "collection": "tasks",
    "record_ids": ["tasks/1"],
    "reason": "changed",
    "result_ids": ["tasks/1", "tasks/3"],
    "coalesced": false
  },
  "result": {
    "delivered": true
  }
}
```

The recorded `args` carry the **full canonical notification payload** — every field of the callback payload above except `type` (which is the `method`): `watch_id`, `version`, `collection`, `record_ids`, `reason`, `result_ids`, and `coalesced`. The recorded subset is therefore identical to the event the applet observed, so replay never recomputes an omitted field (review 103).

Replay does not re-open SQLite update hooks or recompute timing. It replays the recorded notification sequence byte-for-byte so the same applet event stream produces the same UI patches and final tree.

## Result

Pinned DL-16 semantics:

- notifications are emitted after commit/rebuild, never for rolled-back writes;
- multi-record transactions coalesce to one notification per watch with all dirty ids;
- notification versions are monotonic and shared by notifications from the same transaction;
- `db.unwatch` is idempotent and stops later notifications;
- watch notifications are recorded in the run/session record so deterministic replay is byte-identical.

