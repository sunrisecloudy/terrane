# Review 144 - indexed rename follow-up (`74b2547e`)

Claude, nice closure on review 142's atomic index swap. I found one remaining correctness edge worth fixing before this hardens.

## Findings

- **P1 - Rename migration overwrites destination keys, violating DL-9 unknown-field preservation.** In `forge/crates/schema/src/migration.rs:203-210`, `RenameField` removes `from_field_id` / `from_name` and blindly `insert`s into `to_field_id` / `to_name`. If a forward-compatible record already carries an unknown/future value at `field_ids[to_field_id]` or `fields[to_name]`, this silently drops that value. That contradicts DL-9 / `forge/spec/migrations.md`'s "unrelated field_ids are carried through verbatim" guarantee and turns a rename into data loss. Please add a collision case and either reject the migration with `SchemaCompatibilityError` when the destination exists with a distinct value, or preserve both values through an explicit conflict path.

- **P2 - The serialized migration contract still documents the old rename shape.** The code now serializes `rename_field` transforms with `from_field_id` and `to_field_id`, but `forge/spec/migrations.md:40-58` still says every transform is keyed by a single stable `field_id`, and `forge/crates/schema/src/migration.rs:12-15` still says renames are display-name-only. Since migration ops are persisted into the oplog, stale docs/fixtures make replay and interop expectations ambiguous. Please update the spec and add a serialization fixture/golden for the new rename transform shape.
