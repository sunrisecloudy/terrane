# Capability: `query` — JMESPath reads + aggregation pipelines + materialized views

New crate `rust/crates/terrane-cap-query/`, namespace `query`, registered in
`default_registry`. Two modes over the same source model:

1. **JMESPath** — one expression over a source document; pure read, never
   recorded.
2. **Aggregation pipeline** — MongoDB Aggregation Pipeline (subset) over a
   source collection: on-the-fly `$lookup` joins for ad-hoc reads, or
   `query.materialize` to snapshot the result into a named **view** whose rows
   are then readable by key in O(log n).

## Locked decision

**On-demand now, reactive later.** `query.materialize` is a command: it runs
the pipeline over *current folded state* inside `decide` (pure — state in,
events out), and the result is persisted through ordinary events, so replay
rebuilds every view without re-running pipelines against live stores. Event
payloads carry the pipeline definition hash and the source event cursor, and
views are pre-registered in a registry (`query.view.define`), so a v2 reactive
engine (re-materialize via broadcast fold on source events) needs **no format
change** — it only adds a trigger.

## Source model

Every read/pipeline names a source, app-scoped (cross-app is a typed error in
v1):

```jsonc
{ "kv":    { "key": "cart/42" } }            // one JSON value
{ "kv":    { "prefix": "cart/" } }           // → [{ "key": …, "value": … }, …]
{ "table": { "name": "orders", "query": { /* relational_db QueryRequest */ } } }
{ "view":  { "name": "sales-by-day" } }      // a previously materialized view
{ "docs":  [ … ] }                           // inline documents (testing / $lookup)
```

`kv` values that parse as JSON are used as documents; non-JSON strings become
`{"key", "value"}` with the raw string. `table` reuses `relational_db`'s
existing partition/sort-range `QueryRequest` verbatim — no second query dialect
for row selection.

## Mode 1 — JMESPath (pure read)

- Resource: `ctx.resource.query.jmespath(sourceJson, expression)` → JSON
  string result. Also a host query (`query.jmespath`) for CLI/MCP.
- Engine: the `jmespath` crate (pure Rust). It is low-churn; pin it and wrap
  behind our own `eval(expr, value)` so it is swappable. Compliance is proven
  by vendoring the relevant JMESPath compliance-suite JSON fixtures into tests.
- Never recorded, no events — deterministic function of folded state, same
  class as `blob.stat` or kv reads.

## Mode 2 — Aggregation pipeline

### Stage subset (v1)

`$match`, `$project`, `$addFields`, `$unset`, `$unwind`, `$group`, `$sort`,
`$skip`, `$limit`, `$count`, `$replaceRoot`, `$lookup`.

- `$group` accumulators: `$sum`, `$avg`, `$min`, `$max`, `$count`, `$first`,
  `$last`, `$push`, `$addToSet`.
- Expression operators (usable in `$match` via `$expr`, `$project`,
  `$addFields`, `$group`): comparison (`$eq $ne $gt $gte $lt $lte $in`),
  boolean (`$and $or $not`), arithmetic (`$add $subtract $multiply $divide
  $mod $abs $floor $ceil $round`), string (`$concat $toLower $toUpper $substr
  $split $strLen $trim`), array (`$size $arrayElemAt $filter $map $slice`),
  conditional (`$cond $ifNull $switch`), type (`$toString $toInt $toDouble
  $toBool $type`), field paths (`$fieldName`, `$$ROOT`, `$$this`).
- `$lookup` (the on-the-fly join): `{ from: <source object>, localField,
  foreignField, as }` — `from` is any source from the model above (kv prefix,
  table, view), same app. Equality-match v1; pipeline-form `$lookup` is v2.
- Anything outside the subset → typed `InvalidInput` naming the stage/operator
  and the supported list (agents self-correct from that).

### Determinism rules (documented in `doc.rs`, enforced by tests)

- JSON value total order for `$sort`/`$min`/`$max`/`$addToSet` dedup:
  `null < bool < number < string < array < object` (Mongo-style), numbers
  compared as f64, objects by sorted-key canonical form. Sorts are stable.
- `$group` iterates input order; group keys are canonical-JSON strings in a
  `BTreeMap`, so output order is deterministic regardless of input order.
- Numbers: integers stay i64 until an operation forces f64 (`$divide`,
  `$avg`); IEEE-754 ops are deterministic; NaN is a typed error, never a
  stored value.
- Limits: ≤ 32 stages, ≤ 100 000 docs scanned per source, ≤ 10 000 result
  docs, `$lookup` foreign scan ≤ 100 000 — each a typed error, all named in
  the doc.

### Ad-hoc execution (pure read)

`ctx.resource.query.pipeline(sourceJson, pipelineJson)` → JSON array. Same
class as JMESPath: deterministic over state, never recorded.

## Materialized views ("store result for fast query by key later")

### Registry + snapshot commands

| Command | Effect |
| --- | --- |
| `query.view.define` | args `app, view, definition_json` (`{source, pipeline, key}`) → event `query.view.defined { app, view, def_json, def_hash }` (sha256 of canonical def). Redefining bumps the def and orphans old rows on next materialize. |
| `query.materialize` | args `app, view` → runs the registered pipeline over current state **in decide**; emits `query.materialized { app, view, def_hash, source_cursor, row_count }` followed by the row events (below). Erroring pipeline ⇒ no events. |
| `query.view.drop` | event `query.view.dropped { app, view }`; fold clears registry entry + rows. |

`key` in the definition names the result field used as the row key (default
`_id`, i.e. the `$group` key); duplicate keys in a result are a typed error.
`source_cursor` is the last folded event seq at materialize time — the
staleness handle, and the exact field a reactive v2 uses to decide "behind".

### Row persistence

Rows are **query-owned events**, not `kv.set` broadcasts, so the namespace
stays self-contained and replay is exact:

- `query.row.put { app, view, def_hash, key, doc_json }` — one per result row.
- A materialize emits: `query.materialized` (header), then row puts for the
  new snapshot, then implicit clearing — fold of `query.materialized` drops
  all existing rows of that view first (snapshot semantics ⇒ no per-row
  deletes needed).

Fold keeps `app → view → { def, def_hash, source_cursor, rows: BTreeMap<key, doc> }`.
Row count per view ≤ 10 000 (limit above); large snapshots are the reactive
v2's incremental-maintenance problem, deliberately not solved here.

### Reads

| Resource | Returns |
| --- | --- |
| `query.view.get(view, key)` | one doc or null — the fast-by-key path |
| `query.view.scan(view, prefix?, limit?)` | `[{key, doc}]`, key-ordered |
| `query.view.stat(view)` | `{defHash, sourceCursor, rowCount}` — staleness check |
| `query.view.list()` | registered views + stats |

`view` is also a valid pipeline/JMESPath *source*, so views compose.

Reacts to `app.removed`: drop the app's registry and rows.

## Implementation plan

1. **Engine module** (`terrane-cap-query/src/pipeline/`): value ordering +
   canonical JSON, expression evaluator, stage executors, limits. Pure
   functions `Vec<Value> → Result<Vec<Value>>` — no StateStore in sight, so
   the conformance tests are table-driven fixtures (JSON in/out, including
   Mongo-documented examples for each stage).
2. **Source resolver** (`src/source.rs`): source JSON → `Vec<Value>` reading
   kv/relational_db/view state through `QueryCtx`/`CapBus` (read-only).
3. **JMESPath wrapper** (`src/jmespath.rs`) + vendored compliance fixtures.
4. **Capability impl** (`lib.rs`, `doc.rs`): manifest (commands, events,
   resources, `app.removed` subscription), decide (define/materialize/drop —
   materialize composes 1+2), fold (registry + snapshot-replace + rows),
   describe, grant resource `query`.
5. **Register** in `default_registry`; `APP_API.md` (`ctx.resource.query.*`
   with one worked example: kv prefix → `$group` daily totals → materialize →
   `view.get`); scaffold recipe mention; MCP `capability_query` exposure comes
   free from the manifest.
6. **Tests:** engine fixtures (step 1, largest surface); integration
   `terrane-core/tests/cap/query.rs` — define/materialize/read round-trip,
   snapshot-replace on re-materialize, `$lookup` kv⇄table join, view-as-source
   composition, determinism (shuffled kv insertion order → identical events),
   replay identity, limits, app.removed; e2e `terrane-host/tests/cap/query.rs`
   — JS backend using `ctx.resource.query` end-to-end (pure, default-run).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Reactive refresh (v2, format-ready), cross-app sources, pipeline-form
`$lookup`/`$facet`/`$bucket`/`$graphLookup`, incremental view maintenance,
SQL. The `search` cap (separate branch) stays separate: `query` is exact and
structural; `search` is fuzzy and ranked.
