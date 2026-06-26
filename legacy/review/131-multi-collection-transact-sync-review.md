# Review 131: multi-collection transact follow-up

Reviewed commit `a69e4378` (`forge-storage: atomic multi-collection transact + reject non-row watches (review 129)`).

## Findings

- **P1: Cross-collection `transact` is locally atomic but can still sync as a partial transaction.** The new write path correctly keeps every per-collection bucket inside one local `Store::transact`, but it persists one chunk/oplog row per collection for a single logical `record.transact` (`forge/crates/storage/src/crdt_write/mutation.rs:282-338`; the new test asserts two oplog rows for a tasks+notes group at `forge/crates/storage/src/crdt_write/mod.rs:327-333`). The sync layer then stages and authorizes each chunk independently, drops denied chunks one by one, and imports the remaining allowed chunks as a batch (`forge/crates/sync/src/lib.rs:556-570`, `forge/crates/sync/src/lib.rs:590-598`). So if peer A commits `transact([write tasks, write notes])` and peer B is allowed to receive `tasks` but denied `notes`, B will import only the `tasks` chunk, creating a state that no peer locally committed. That contradicts DL-17's requirement that `transact([...])` is "merged as a unit" (`prd-merged/02-data-layer-prd.md:68-70`). Please either add transaction-group metadata so sync authorizes/applies all chunks from the same logical transaction all-or-nothing, or keep cross-collection transact unsupported until the sync/apply boundary can preserve the unit. Add a `forge-sync` regression where one collection in a mixed transaction is denied and assert neither collection lands.

## Verification

- `cargo test -p forge-storage --lib crdt_write`
- `cargo test -p forge-storage --lib watch`
- `cargo test -p forge-storage --test live_query_fixtures`
- `cargo test -p forge-sync`
- `cargo clippy -p forge-storage --tests -- -D warnings`
