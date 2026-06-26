# Review 005 - schema commit 78db152

Buddy review for Claude on `78db152 forge-schema: dynamic schema registry with stable ids + additive-only evolution`.

## Findings

- **P1 - Field IDs collide under offline concurrent schema edits.** `CollectionDef::add_field` mints `f{next_field_seq}` per collection (`forge/crates/schema/src/collection.rs:52`, `forge/crates/schema/src/collection.rs:96`), but the merged PRD requires stable IDs with per-actor ranges (`actor-id ⊕ counter`) so schema changes can sync as CRDT data (`prd-merged/02-data-layer-prd.md:15`, `prd-merged/02-data-layer-prd.md:57`). Two offline actors adding the first field to the same collection both produce `f0`, and `validate_compatibility` cannot distinguish a true rename from an ID collision if the type is compatible. Please switch minted IDs to actor-scoped IDs or make the temporary linearized allocator impossible to use for synced/offline schema changes.

- **P1 - Record validation ignores authoritative stable field IDs.** `RecordEnvelope` marks `field_ids` as authoritative for merge (`forge/crates/domain/src/record.rs:30`) and PRD DL-7 says renames touch only the display name, but `SchemaRegistry::validate_record` checks required fields and unknown fields only through `record.fields` display names (`forge/crates/schema/src/registry.rs:250`, `forge/crates/schema/src/registry.rs:279`). After a field rename, an old record carrying `field_ids["f0"]` can be warned/rejected as missing the renamed display field, while unknown stable IDs in `record.field_ids` are never surfaced as DL-9 warnings. Please validate presence/unknowns by `field_id` first, then treat display names as an applet-facing projection.

- **P2 - Public mutable schema internals bypass the additive-change API.** The crate says destructive operations have no API surface, but `SchemaRegistry.collections`, `CollectionDef.fields`, `CollectionDef.next_field_seq`, and every `FieldDef` field are public (`forge/crates/schema/src/registry.rs:79`, `forge/crates/schema/src/collection.rs:49`, `forge/crates/schema/src/collection.rs:17`). External crates can remove fields, rewind `next_field_seq`, or mutate types without going through `SchemaChange`. Please consider private fields plus read-only accessors/builders, or make deserialized registries run invariant validation before use.

## Verification

- `cargo test --locked` from `forge/`: passed.
- `cargo test --locked -p forge-schema` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-schema` from `forge/`: passed.
- `cargo clippy --locked -p forge-schema -- -D warnings` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown` from `forge/`: still fails globally on `rquickjs-sys` and `sqlite-wasm-rs`; not introduced by this commit, but still blocks the full Rust/WASM lane.
