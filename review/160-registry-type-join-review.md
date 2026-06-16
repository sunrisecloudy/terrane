# Review 160 - registry union type join

Reviewed commit `b344f51d` (`forge-schema/storage: deterministic field-union registry merge on migration import (DL-13 review 159)`).

## Finding

- **P1 - Incompatible-type tie-break can narrow one valid branch.** `FieldDef::merge_with` handles two same-`field_id` types that do not directly widen to each other by picking the larger `FieldType::order_key` (`forge/crates/schema/src/collection.rs:124-137`). That is deterministic, but it is not necessarily a common supertype. A valid DL-8 split can produce this today: from an `IntNum` base, one peer widens to `FloatNum` and another widens to `Nullable(IntNum)`; both are legal under `can_widen_to` (`forge/crates/schema/src/field_type.rs:100-105`), but neither target directly widens to the other. The new tie-break chooses `Nullable(IntNum)` because nullable sorts last, which is not a widening of `FloatNum`, so data migrated by the float branch is now described by a narrower schema after sync. Please replace the order-key winner with a real least-upper-bound/join for field types (for this example, `Nullable(FloatNum)`; similarly `Scalar` + `Nullable(T)` should become `Nullable(Scalar)`), and fail closed only when no additive common supertype exists. Add a regression where two peers widen the same `IntNum` field to `FloatNum` and `Nullable(IntNum)` and assert the merged type is `Nullable(FloatNum)`.

Checks run:

- `cargo test -p forge-schema merge`
- `cargo test -p forge-sync migration_chunk`
