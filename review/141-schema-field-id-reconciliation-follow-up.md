# Commit Review: 1d14bbeb

Reviewed commit: `1d14bbeb forge-core: reconcile migration field ids + drive rename (DL-13 M2, review 140)`

## Findings

### P1 - Indexed default fields split identity from later DL-4 writes

The fix makes indexed `add_field` defaults fill existing records under the registry id (`field.field_id()`, e.g. `f_fx_1`) so the newly created index at `forge/crates/core/src/commands/schema.rs:85` is populated (`forge/crates/core/src/commands/schema.rs:427`, `forge/crates/core/src/commands/schema.rs:449`). But the applet/CRDT write path still has no registry name-to-id map: `materialize_field_ids` always layers display writes under `f_<name>` (`forge/crates/storage/src/records.rs:114`), and CRDT projection materialization writes the envelope back exactly (`forge/crates/storage/src/crdt_write/rebuild.rs:78`, `forge/crates/storage/src/crdt_write/crdt_encoding.rs:55`).

That means the regression test only proves the default value is initially queryable. As soon as the applet later patches `{ "priority": 5 }`, the record keeps stale `field_ids["f_fx_1"] = 0` and gains/updates `field_ids["f_priority"] = 5`. Queries and indexes by the advertised schema id continue to see the old default, while display reads see the new value. The same split also loses pre-existing unknown/display data during schema adoption: a record already carrying `priority: 5` gets a default under `f_fx_1` instead of treating the existing `f_priority` value as the field's value.

Suggested fix: complete the registry-id path end-to-end before indexing registry ids. Either keep M0a indexes on the `f_<name>` stand-in until storage materialization is registry-aware, or plumb the live schema mapping into DL-4/CRDT materialization so writes to display `priority` update `f_fx_1` and migrate/copy any pre-existing `f_priority` value. Add a regression after the current test's default backfill: patch the display field to a non-default value, rebuild projection, then assert `field_id: "f_fx_1"` queries find the new value and no longer find the old default.

## Verification

Not run; static heartbeat review only, with unrelated dirty worktree changes preserved.
