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
    { "op": "add_field",    "field_id": "f_alice_2", "name": "currency", "default": "USD" },
    { "op": "rename_field", "from_field_id": "f_amount", "to_field_id": "f_amount_total", "from_name": "amount", "to_name": "amount_total" },
    { "op": "drop_field",   "field_id": "f_alice_1", "name": "note" },
    { "op": "widen_field",  "field_id": "f_alice_0", "name": "amount", "to": "float_num" }
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

Every transform that touches the display projection carries the prior display
`name` (or `from_name`/`to_name` for a rename) **explicitly** — never inferred by
stripping the `f_` stand-in prefix off the `field_id`. A schema-minted id like
`f_alice_1` has an unrelated display name (e.g. `note`), so a guess would clean the
wrong key (review 138 P2).

| op             | effect on each record                                                            |
| -------------- | -------------------------------------------------------------------------------- |
| `add_field`    | if the record lacks `field_id`, set it to `default` (a constant JSON value); records that already carry the field are left untouched. The display `name` is also written into `fields[name]` so the projection stays readable. |
| `rename_field` | MOVES BOTH the name-derived record-side stand-in `field_ids[from_field_id]` → `field_ids[to_field_id]` AND the display projection `fields[from_name]` → `fields[to_name]`. Under the M0a `f_<name>` stand-in scheme the record-side id is name-derived, so moving only the display key would strand the value under `f_<old_name>` while the index / write path / query all moved to `f_<new_name>` — a split identity (review 142). All four keys (`from_field_id`/`to_field_id`/`from_name`/`to_name`) are explicit; `from_field_id == to_field_id` (a same-name rename) moves nothing, and a record not carrying the old keys is a no-op (idempotent after the move). **DL-9 destination-collision guard (review 144):** if the record already carries a value at the rename DESTINATION (`field_ids[to_field_id]` or `fields[to_name]`) that is DISTINCT from the one being moved — a forward-compatible/unknown value a peer wrote ahead of this rename — the migration is REJECTED with `SchemaCompatibilityError` (atomic, so it rolls back) rather than silently overwriting it; a destination equal to the moved value (or absent) proceeds. |
| `drop_field`   | remove the field from both `field_ids[field_id]` and the display projection at `name`. This is the *data* side of DL-8 "deprecate + retain at the schema level": the record value is dropped, but the migration is recorded in the oplog so the drop is replayable, never a destructive `ALTER TABLE`. |
| `widen_field`  | coerce the stored value to the wider type, in BOTH `field_ids[field_id]` and the display projection at `name`. Only **widening** coercions are legal (`int_num → float_num`, any scalar → `scalar`, `T → nullable(T)`); the value is rewritten to its widened JSON form (e.g. integer `5` → float `5.0`). |

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
  deprecate).
- **A duplicate `add_field` for the same `field_id`** is a `ValidationError`
  (`MigrationDescriptor::validate`): `add_field` is fill-if-missing, so two adds for
  one id would make the result depend on which ran first, breaking the §2 determinism
  contract. Structural validation (empty collection, non-advancing version, duplicate
  add) is the only up-front rejection the *pure* descriptor can perform — it has no
  registry, so a transform that targets a `field_id` no record carries is simply an
  idempotent **no-op** (nothing to widen/drop/rename), not an error. The *runtime*
  precondition (current `schema_version == from`) is checked by the storage driver.

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
2. For **every** record in `descriptor.collection` (read from the CRDT doc — the
   source of truth — in sorted id order), apply `migrate_record` and write the
   migrated envelope back **into the CRDT doc**. The first record that cannot be
   transformed (e.g. a lossy narrow) returns its typed error, which propagates out
   of the closure. The doc is committed once and the new ops are exported as **one
   immutable `crdt_chunks` row** — so the migration lives in the same CRDT stream a
   DL-6 rebuild replays.
3. Materialize each migrated record into the derived `records` projection (FTS-synced).
4. Append one `schema.migration` op to the oplog (the DL-13 "migrations are oplog
   operations" requirement): `{from, to, collection, transforms, record_ids}`.
5. Bump the persisted `schema_version` to `to_schema_version`.
6. Rebuild active indexes from the migrated projection (DL-8 → DL-5/DL-6).

Because all steps run in the single transaction, **any** failure — a non-coercible
record at step 2, an index failure at step 6, anything — rolls back the WHOLE
migration: the migration chunk, `schema_version`, every transformed record, the oplog
op, and the indexes are left **exactly** as they were before the call. There is no
partial migration and no destructive DDL.

**Durability under rebuild (DL-6).** Crucially the migration mutates the **CRDT
source of truth**, not just the derived projection. A DL-6
`rebuild_projection` drops the `records` table and rematerializes it purely from
`crdt_chunks`; because the migration appended its chunk to that stream, a rebuild
reproduces the **migrated** values with zero diff — it does not silently restore the
pre-migration state while leaving `schema_version` advanced (review 138 P1). On a
rolled-back migration no chunk is appended, so a rebuild reproduces the pre-migration
state exactly. Two tests pin this: `migration_failure_rolls_back_everything` (forces
a mid-stream failure and asserts the version, the records, the oplog, AND that no CRDT
chunk survives) and `migration_survives_dl6_projection_rebuild` (applies a migration,
rebuilds the projection from chunks, and asserts the migrated values + `schema_version`
+ oplog + indexes remain coherent).

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

## 6. Migrations sync to peers (SS, review 139)

A migration chunk is an ordinary append-only `crdt_chunks` row on the collection doc,
so it rides the SS-1/SS-2 chunk-diff sync seam like any record write — but two extra
pieces of metadata make it FIRST-CLASS on the sync path rather than dropped at a peer:

1. **A per-chunk oplog row** keyed `collection/<name>#chunk-NNNN` (the SAME scheme an
   ordinary mutation chunk uses) is written alongside the chunk, carrying the migrated
   `record_ids` AND the `from`/`to` schema versions. The sync seam joins chunks →
   metadata by exactly this `{doc_id}#{chunk_id}` op id, so the migration chunk is
   DISCOVERABLE (`missing_chunks_for_doc`) and AUTHORIZED as a record write against the
   migrated record ids — not denied as a record-less write. The separate
   `schema.migration` AUDIT row (keyed `migration#<from>-<to>#<collection>`) is still
   written; only the per-chunk row participates in the sync join.

2. **The receiver advances its `schema_version`** to the chunk's carried `to` value on
   an authorized import, IN THE SAME transaction as the chunk insert + projection
   rebuild (`apply_remote_chunks`). The advance is monotone and idempotent: a receiver
   already at or beyond `to` is left unchanged (a converged peer, or one that migrated
   locally), never an error, so a re-sync stays a pure no-op. Because it is bound to the
   import txn, a receiver can never materialize migrated record values while staying
   behind at the old `schema_version` — no version drift — and a failed import (e.g. a
   rebuild rejection) rolls the version advance back with the chunk.

   **Metadata survives EVERY relay hop (review 145).** A migration is a schema-affecting op
   at every hop, not just the first. When a peer B IMPORTS a migration chunk from A, its
   `record.remote_import` oplog row carries the schema-affecting metadata FORWARD — the
   target `to` version, an explicit `is_migration` marker, and the evolved
   `registry_collection` — so when B RELAYS the chunk to C the sync seam recovers it from
   B's remote-import row (not only from an authoring peer's `schema.migration` row) and
   re-stages the chunk as a migration. Therefore C advances its `schema_version` + registry
   exactly like a direct receiver, and the `schema_write` gate (§7) is RE-APPLIED at the
   B→C hop too (it is never bypassed because B is merely relaying). FAIL-CLOSED: if a
   forwarded row is marked schema-affecting but its `to` target is unrecoverable, the seam
   stages it `malformed` and the apply boundary DENIES it rather than importing the migrated
   data as a plain record write that would silently skip the schema advance. Before this, a
   relay's row dropped the version/registry and the next hop imported migrated DATA while
   staying at the old `schema_version` under an unevolved registry (C inconsistent).

3. **The receiver evolves its schema registry in lockstep** (review 143). The per-chunk
   migration row also carries the affected collection's EVOLVED registry entry
   (`registry_collection = { name, collection }`, the post-change `CollectionDef`). On an
   authorized import — and ONLY when the version actually advances — the receiver replaces
   that collection's entry in its persisted `SchemaRegistry`, re-validates, and writes it
   back IN THE SAME transaction as the chunk insert + `schema_version` advance. So after
   sync the receiver's records, `schema_version`, registry, AND indexed-field
   reconstruction (`collection_indexed_fields` keyed by `f_<name>`) all agree; the receiver
   never sits at version N with N-shaped data under an N-1 (or empty) registry. The
   in-memory registry handle + index manager are refreshed from the store after sync (see
   `WorkspaceCore::sync_with`). The registry-as-CRDT-document direction (prd-merged/02:15)
   is realized in M0a by carrying the migration's registry change with the chunk; a full
   registry-as-CRDT vector merge (DL-11) is future work. A migration whose `from` version
   does not match the receiver advances nothing (the monotone `to`-newer guard), so the
   version never moves ahead of the registry.

## 7. Migration authorization is a SCHEMA CHANGE (SS-7, review 143)

A migration chunk is NOT a plain record write on the sync path: it advances the
receiver's `schema_version` and evolves its registry, so it is a SCHEMA-AFFECTING op.
The receiver's authorizer (`sync-rbac.md`) gates it as a schema change — it requires
BOTH the collection `db.write` grant AND schema-change authority (Owner/Maintainer role
+ `schema_write = true`), exactly as a schema change at the command boundary does. An
Editor trusted with only `db.write` on the collection may author ordinary record writes
there, but a migration chunk it sends is DENIED — fail-closed, before any chunk import,
version advance, or registry evolution. The schema-affecting signal is carried through
the authorization envelope (the chunk's `schema_version`), never dropped, so the gate
sees a migration as a migration. (This closes the review-143 RBAC bypass, where every
synced chunk — including a migration — was translated to a plain `db.write` record op
and an unauthorized Editor could bump a peer's schema.)
