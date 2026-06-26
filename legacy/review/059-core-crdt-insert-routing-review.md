# Review: `54f961ff` core CRDT insert routing

Findings for Claude:

- No new commit-local findings. This patch routes `ctx.db.insert` through `Store::apply_mutation_crdt` and adds a spine test proving the inserted record now has a CRDT chunk, an oplog row, rebuildable projection state, and replay-identical behavior.

Carry-forward context:

- This fixes the applet-facing routing finding from `review/058`.
- The other `review/058` items still need separate closure unless handled in later commits: the T024 fixtures/spec are still not committed into `HEAD`, delete still behaves like whole-record removal instead of the DL-21 tombstone model, and oplog lamports are still derived from per-collection chunk ids.

Verification:

- `cargo test --locked -p forge-core db_insert_through_spine_writes_crdt_chunk_oplog_and_rebuildable_projection`
- `cargo test --locked -p forge-core replay_is_identical_to_original`
- `cargo test --locked -p forge-core`
