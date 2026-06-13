# Query DSL and Mutation Vectors

Source of record: `prd-merged/02-data-layer-prd.md` DL-15, DL-16, DL-17; command shell binding: `forge/spec/commands.md` `query.execute`; data-browser consumer: `prd-merged/05-ui-system-prd.md` UI-17.

This document pins the v1 query surface before the Rust planner lands. The applet API is typed and capability checked; the SQL-like string form is for `query.execute`, the data browser, and SDK tooling only. Raw SQL is never exposed to applets.

## Data Model

Queries read the rebuildable `records` projection. Each row is a `RecordEnvelope` with:

- `collection` and `entity_id` as the logical table/id;
- `fields` keyed by display names for applet ergonomics;
- `field_ids` keyed by stable schema field ids for merge/index correctness;
- `deleted` tombstones hidden from normal queries unless `includeDeleted` is explicit.

The planner resolves display field names through the schema registry, then executes against stable `field_ids` when available. Unknown collections remain readable through the data browser per DL-10, but applet calls still require matching `db.read` capability.

## Applet DSL

Canonical applet form:

```ts
const rows = await ctx.db
  .from("tasks")
  .where(f => f.prio.gt(1).and(f.done.eq(false)))
  .orderBy("prio", "desc")
  .limit(20)
  .offset(0)
  .all();
```

M0a planner subset:

| Surface | Status | Notes |
|---|---|---|
| `from(collection)` | M0a | One collection, capability checked with `db.read`. |
| `where(field.eq(value))` | M0a | JSON scalar equality. |
| `ne`, `lt`, `le`, `gt`, `ge` | M0a | No type coercion. Mixed-type comparisons are rejected. |
| `in(values[])` | M0a | `values` must be a non-empty array of JSON scalars. |
| `like(pattern)` | M0a | SQL LIKE pattern, `%` and `_`, backslash escape. |
| `and`, `or` | M0a | Explicit boolean nodes; no implicit precedence in JSON plan form. |
| `orderBy(field, "asc" | "desc")` | M0a | Stable secondary order by `entity_id`. |
| `limit(n)`, `offset(n)` | M0a | `0 <= n`; planner caps `limit` to the grant max when present. |
| `count`, `sum`, `avg`, `min`, `max` | M0a | Aggregates over one collection. |
| `groupBy(field)` | M0a | One group key for v1 spine vectors. |
| `text(field).match(query)` | P1 | FTS5-backed text search; LIKE stays M0a. |
| `join(other).on(refField, other.id)` | P1 | Workspace-local joins on declared reference fields only. |
| `watch(query, cb)` | P1 | DL-16 live query; same query AST as `all()`. |

## SQL-Like String Form

`query.execute` accepts the same validated subset as the DSL:

```sql
SELECT id, title, prio
FROM tasks
WHERE prio > 1 AND done = false
ORDER BY prio DESC
LIMIT 20 OFFSET 0
```

Allowed grammar:

- `SELECT *`, `SELECT id`, display fields, or allowed aggregates.
- `FROM <collection>`.
- `WHERE` with `=`, `!=`, `<`, `<=`, `>`, `>=`, `IN (...)`, `LIKE`, `AND`, `OR`, and parentheses.
- `GROUP BY <field>` when every non-aggregate selected field is grouped.
- `ORDER BY <field> ASC|DESC`.
- `LIMIT <integer>` and `OFFSET <integer>`.
- P1 only: `MATCH` for FTS and `JOIN ... ON` for declared references.

Rejected as raw SQL or outside the subset: `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`, `PRAGMA`, comments, semicolons, subqueries, CTEs, arbitrary functions, wildcard table names, and unbound parameters. The planner compiles accepted strings into its internal query AST and then into parameterized SQLite; it never executes caller SQL directly.

## Mutations

Applet mutation APIs:

```ts
await ctx.db.insert("tasks", { title: "Ship", prio: 3 });
await ctx.db.update("tasks", "tasks/1", { title: "Ship v1", prio: 3 });
await ctx.db.patch("tasks", "tasks/1", { done: true });
await ctx.db.delete("tasks", "tasks/1");
await ctx.db.transact([
  ctx.db.insert("tasks", { title: "A" }),
  ctx.db.patch("tasks", "tasks/1", { done: true })
]);
```

Mutation rules:

- Every mutation requires `db.write` capability for the target collection.
- `insert` creates one CRDT op and one projection row; caller-supplied ids are optional but must be collection scoped.
- `update` replaces known display fields but preserves `field_ids`, `unknown_fields`, and `extensions` not mentioned by the caller.
- `patch` merges the supplied fields into the current record.
- `delete` marks the record tombstoned; hard purge is not in the applet API.
- `transact([...])` commits all included mutations as one local SQLite transaction and one CRDT commit; failure rolls back the group.

## Result

Pinned semantics for the initial planner:

- Comparisons do not coerce types. `"2" > 10` is a validation error, not false.
- Missing fields compare as JSON null only for `eq(null)` / `ne(null)`; range comparisons on missing/null fields are false.
- Sort order is numbers, strings, booleans, nulls last; `entity_id` is the stable tie-breaker.
- `LIKE` uses `%` and `_`, backslash escapes those metacharacters, and is ASCII case-insensitive to match SQLite's portable default.
- P1 FTS and join fixtures are included to pin semantics but should be allowed to report `unsupported_feature` until those planner paths land.

Open questions for Claude/Rust planner:

- Whether M0a should add an explicit `isNull` operator instead of overloading `eq(null)`.
- Whether non-ASCII LIKE case folding must be custom rather than SQLite-default.
- Whether `update` should remove omitted display fields or behave as a full replacement only after schema validation grows delete-field tombstones.
