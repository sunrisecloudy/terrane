# Dynamic Indexes

Source of record: `prd-merged/02-data-layer-prd.md` DL-5/DL-6 plus
`prd-merged/DECISIONS.md` E3. This spec maps the dynamic-index promise onto the
committed `forge-storage` projection:

```sql
records(collection TEXT NOT NULL, id TEXT NOT NULL, data TEXT NOT NULL, updated_at INTEGER,
        PRIMARY KEY(collection, id))
```

`records.data` is the canonical JSON `RecordEnvelope`, so the PRD shorthand
`json_extract(data,'$.<field_id>')` resolves to the stable-id envelope path
`json_extract(data,'$.field_ids.<field_id>')`. Display names under `$.fields`
are readable projection sugar; planner/index matching must use stable field ids.

## Index Definitions

The registry is the logical owner of index intent. A live field with
`indexed = true` produces an index definition for that `(collection, field_id)`.
Deprecated fields may retain old data, but their indexes are not planner
candidates.

Index definitions are rebuildable metadata, not data sources:

- `collection`: logical collection name, matching `records.collection`.
- `field_id`: stable field id from `forge-schema`.
- `kind`: `expression` for equality/range/order over JSON1 values, or `fts5`
  for full-text search over text fields.
- `state`: lifecycle state below.
- `definition_hash`: deterministic hash of the canonical definition
  `(collection, field_id, kind, expression, tokenizer/options)`.

Current `forge-schema` has only `FieldDef.indexed`; it does not yet expose a
separate `full_text` flag or an `AddIndex` mutation. Until those land, fixtures
carry `indexes[].kind` explicitly while keeping the final schema field marked
`indexed = true`.

## Expression Index DDL

For an indexed scalar/text/numeric field, the engine emits one SQLite expression
index scoped to the collection:

```sql
CREATE INDEX IF NOT EXISTS "idx_records_tasks_f_alice_1"
ON records (json_extract(data, '$.field_ids.f_alice_1'))
WHERE collection = 'tasks';
```

Normative rules:

- The index name is deterministic and must be quoted. Implementations may use a
  short hash in the name when a collection or field id is too long for readable
  names, but the same definition must produce the same name on every platform.
- The JSON path is built from the stable `field_id`, not the display name.
- The partial predicate must at least constrain `collection = <collection>`.
  Query SQL must include the same collection predicate for SQLite to consider
  the partial index.
- Equality and range predicates must use the same expression text as the DDL.
  A planner may add type casts only if the DDL and generated query use the same
  cast. The M0a vectors avoid casts and rely on JSON1 numeric values for range.
- Missing values index as NULL. Query semantics decide whether NULL matches;
  the index manager must not synthesize defaults.
- `records` remains canonical. Dropping or corrupting an expression index cannot
  change query answers; it can only change performance and warnings.

## FTS5 Shadow Table

Full-text fields use an FTS5 virtual table maintained from `records.data`.
The FTS table is a derived shadow structure, not canonical storage.

Example:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS "fts_records_notes_f_alice_0"
USING fts5(record_id UNINDEXED, value, tokenize = 'unicode61');
```

Maintenance rules:

- `record_id` stores `records.id`; `value` stores the text value extracted from
  `$.field_ids.<field_id>`.
- Inserts, updates, and deletes to the records projection must refresh the FTS
  row in the same logical write transaction when the index is active.
- A text-search query may use the FTS table only when the definition is active
  and its tokenizer/options match the query requirement.
- FTS rows are rebuilt from current canonical records during DL-6 rebuild; they
  are never exported or trusted as source data.

## Lifecycle

The full DL-5 lifecycle is:

| State | Meaning | Planner may use? |
|---|---|---|
| `proposed` | Registry/index definition exists; no complete physical index is ready. | no |
| `building` | Initial build is in progress for a newly proposed index. | no |
| `active` | Physical index exists, definition hash matches, and all current records are covered. | yes |
| `stale` | Definition or canonical records changed in a way that invalidates coverage. | no |
| `rebuilding` | A stale or after-the-fact index is being rebuilt from canonical records. | no |
| `deprecated` | Field/index is hidden for new use but retained for compatibility/cleanup. | no |
| `removed` | Physical index/shadow table has been dropped and metadata is gone. | no |

Allowed transitions:

- `proposed -> building -> active`
- `active -> stale -> rebuilding -> active`
- `proposed -> rebuilding -> active` when records already exist and the engine
  chooses to use one rebuild path for both initial and later creation.
- `building -> building` and `rebuilding -> rebuilding` on resume; build steps
  must be idempotent and interruptible.
- `proposed|building|active|stale|rebuilding -> deprecated`
- `deprecated -> removed`

The planner must treat every non-`active` state as unavailable. It may still
return correct rows by scanning the records projection, but it must surface a
warning when a query predicate/search/order expected an index.

## Rebuild From Canonical Data

DL-6 requires `forge db rebuild` to reconstruct the projection and indexes
purely from CRDT documents and the schema registry.

Rebuild order:

1. Replay CRDT chunks/snapshots into canonical records.
2. Replace the `records` projection deterministically.
3. Recompute index definitions from the rebuilt registry.
4. Drop/recreate expression indexes and FTS5 tables from `records`.
5. Compare query answers against reference full scans in CI soak; indexes must
   have zero semantic diffs.

The rebuild path must not read previous expression-index pages or FTS rows as
input. If an index is corrupt, missing, stale, or half-built, rebuilding from
canonical records is sufficient to recover it.

## Full-Scan Warnings

The planner must emit a warning with code `planner.full_scan` whenever it falls
back to scanning `records` for a predicate, sort, or text search that is not
covered by an active index.

The warning payload should include:

- `collection`
- `field_id` or display field name when no stable id is available
- `reason`: `no_index`, `index_not_active`, `index_deprecated`,
  `unsupported_operator`, or `fts_not_available`
- `estimated_rows` when known

Primary-key reads by `(collection, id)` use the table primary key and do not
need a dynamic-index warning. Tiny scans may be acceptable, but they still
report the warning so applets and the data browser can expose planner behavior.

## Fixture Shape

Fixtures in `forge/fixtures/indexes/*.json` are planner/rebuild vectors, not
runtime conformance vectors. Each case contains:

- `schema`: final registry intent, usually as `changes` plus a resolved
  collection/field view.
- `indexes`: derived index definitions and lifecycle states.
- `records`: canonical `RecordEnvelope` rows to place in `records`.
- `query`: a normalized query shape over stable field ids.
- `expected`: `uses_index`, optional `index_id`, warnings, and result rows.

`rebuild_after_records.json` also includes phases showing that records existed
before the index was proposed. That vector pins the DL-6 rebuild path even
though the committed `SchemaChange` enum does not yet have an `add_index` op.

## Result

M0a-first states for the Rust index manager are `proposed`, `rebuilding`, and
`active`. That is enough to cover an index definition appearing, an idempotent
build/rebuild from canonical `records`, and planner use only after completion.

The implementation should also recognize `deprecated` as a non-usable state
because the fixture corpus includes a deprecated index that must be ignored.
Full management of `building` as a distinct initial-build state, automatic
`stale` detection queues, resumable progress checkpoints, and physical
`removed` garbage collection are deferrable beyond M0a.
