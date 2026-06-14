# Schema migrations (DL-13) — deterministic atomic record transforms

> T033 spec. The behavioral contract is `forge/fixtures/migrations/` (15 vectors)
> plus the unit/integration tests in `forge-schema` and `forge-storage`.

prd-merged/02-data-layer-prd.md DL-13: **"Logical migrations are oplog operations,
never destructive SQLite DDL. Lens transforms (breaking changes) deferred to v2;
registry reserves `lenses[]` now."**

A *schema change* (DL-7/DL-8, `forge-schema::SchemaChange`) evolves the **registry**
(add collection/field, widen type, deprecate, enforce). A *migration* (DL-13, this
spec) is the companion step that rewrites the **stored records** to match a registry
evolution, deterministically and atomically. The two are layered: the registry
change decides *what the schema is*; the migration decides *how existing data is
carried forward*. This spec covers only the second.

## 1. Descriptor shape

A migration is described by a `MigrationDescriptor`:

```jsonc
{
  "collection": "expenses",
  "from_schema_version": 1,
  "to_schema_version": 2,
  "transforms": [
    { "op": "add_field",   "field_id": "f_alice_2", "name": "currency", "default": "USD" },
    { "op": "rename_field", "field_id": "f_alice_0", "name": "amount_total" },
    { "op": "drop_field",  "field_id": "f_alice_1" },
    { "op": "widen_field", "field_id": "f_alice_0", "to": "float_num" }
  ]
}
```

- `collection` — the single collection whose records this migration rewrites.
- `from_schema_version` / `to_schema_version` — the migration applies iff the
  workspace's current `schema_version == from_schema_version`; it then advances the
  persisted version to `to_schema_version`. `to` must be `> from` (a migration only
  moves the schema forward; equal versions are the idempotent no-op below).
- `transforms[]` — an **ordered** list of per-field transforms, each tagged by
  `op` (serde `tag = "op"`, snake_case), keyed by the **stable `field_id`** (DL-7),
  never the display name. Order is significant and is part of the deterministic
  contract (§3).

### Supported transforms

| op             | effect on each record                                                            |
| -------------- | -------------------------------------------------------------------------------- |
| `add_field`    | if the record lacks `field_id`, set it to `default` (a constant JSON value); records that already carry the field are left untouched. The display `name` is also written into `fields[name]` so the projection stays readable. |
| `rename_field` | DL-7: changes only the display `name` projection; the stable `field_id` value is untouched. A record that carries the value by stable id needs no value move. |
| `drop_field`   | remove the field from both `field_ids[field_id]` and its display projection. This is the *data* side of DL-8 "deprecate + retain at the schema level": the record value is dropped, but the migration is recorded in the oplog so the drop is replayable, never a destructive `ALTER TABLE`. |
| `widen_field`  | coerce the stored value to the wider type. Only **widening** coercions are legal (`int_num → float_num`, any scalar → `scalar`, `T → nullable(T)`); the value is rewritten to its widened JSON form (e.g. integer `5` → float `5.0`). |

### Rejected transforms (typed errors)

- **Narrowing** (`float_num → int_num`, `scalar → int_num`, dropping `nullable`):
  rejected with `SchemaCompatibilityError` *before any record is touched* when the
  type relation itself is a narrowing. A `float_num → int_num` whose stored value is
  **non-integral** (e.g. `12.5`) is rejected with `SchemaCompatibilityError` while
  transforming that record — and because the migration is atomic (§4) the *entire*
  migration rolls back. (The fixtures express the lossy case as
  `narrow_float_to_int_rejected`.)
- **`rename` that is "not expressible"** and **`add_index` / `remove_field` at the
  schema level**: these are registry concerns the linear M0a `SchemaChange` enum has
  no variant for; the fixtures mark them `not_expressible_*`. The migration descriptor
  deliberately does NOT add destructive/registry ops — `drop_field` here drops a
  record *value*, it does not remove a *schema* field (DL-8 keeps fields via
  deprecate). A descriptor that references an unknown `field_id` for a
  rename/drop/widen is a `ValidationError`.

## 2. Deterministic-transform contract

A migration is a **pure function** of `(prior record, descriptor)`:

```
migrate_record(prior: RecordEnvelope, descriptor: &MigrationDescriptor)
    -> Result<RecordEnvelope>
```

- **Determinism / replay-safety.** The same prior record and the same descriptor
  always produce a **byte-identical** migrated record (canonical JSON via
  `serde_json` over `BTreeMap`, so key order is stable). No clocks, no RNG, no
  iteration-order dependence. This makes a migration content-addressable and safe to
  replay during a DL-6 rebuild: re-running the recorded migration op over the same
  inputs yields the same output.
- **Idempotence per transform.** `add_field` only fills a missing value;
  `widen_field` is reflexive (a value already in the wider form is unchanged);
  `rename_field`/`drop_field` are set/remove operations. Applying the produced
  descriptor twice to the same record is a no-op after the first application.
- **Unknown-field preservation (DL-9).** Fields the descriptor does not mention —
  including `unknown_fields` and unrelated `field_ids` — are carried through verbatim.
- **`envelope_version`, `entity_id`, `collection`, `created_at`, `deleted` are
  preserved.** A migration rewrites field *values/types*, not record identity or
  lifecycle. `updated_at` is left as-is (a migration is a system rewrite, not a user
  edit), keeping the transform a pure function of its inputs.

## 3. Atomicity / rollback (all-or-nothing)

`Store::apply_migration(descriptor, indexes)` performs, inside **one**
`Store::transact` (DL-4):

1. Read the persisted `schema_version`. If it already equals `to_schema_version`,
   return the **idempotent no-op** (`applied: false`) without touching anything.
   If it does not equal `from_schema_version`, reject with `SchemaCompatibilityError`
   (the migration's precondition is unmet).
2. For **every** record in `descriptor.collection` (ordered by id), apply
   `migrate_record`. The first record that cannot be transformed (e.g. a lossy
   narrow) returns its typed error, which propagates out of the closure.
3. Write each migrated record back to the projection.
4. Append one `schema.migration` op to the oplog (the DL-13 "migrations are oplog
   operations" requirement): `{from, to, collection, transforms, record_ids}`.
5. Bump the persisted `schema_version` to `to_schema_version`.
6. Rebuild active indexes from the migrated projection (DL-8 → DL-5/DL-6).

Because all six steps run in the single transaction, **any** failure — a non-coercible
record at step 2, an index failure at step 6, anything — rolls back the WHOLE
migration: `schema_version`, every transformed record, the oplog op, and the indexes
are left **exactly** as they were before the call. There is no partial migration and
no destructive DDL (`records` is a projection; the canonical CRDT chunks are never
mutated by a migration, so a rebuild reproduces the pre-migration state on rollback).
A fault-injection test (`migration_failure_rolls_back_everything`) forces a mid-stream
failure and asserts the version, the records, and the oplog are unchanged.

## 4. Idempotence (already-applied)

Re-applying a migration whose `to_schema_version` is already the current version is a
no-op: it reads the version, sees it is already at the target, and returns
`{ applied: false, schema_version: <current> }` without reading or writing records,
the oplog, or indexes. This makes the apply call safe to retry (e.g. after a crash
between commit and the caller observing success).

## 5. Schema version

The workspace `schema_version` is a single monotone `u64` persisted in the
`__forge/meta` KV namespace under `schema_version` (default `1` when absent — every
workspace starts at schema version 1). It is the migration ordering anchor: a
migration moves it `from → to`, and the sync envelope's `schema_version` field
(SS, `sync-rbac.md`) reads it. Migrations only ever advance it.
