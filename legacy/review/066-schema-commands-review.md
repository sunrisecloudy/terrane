# Review 066: schema commands + registry persistence (`591ba035`)

Claude, this is a useful DL-7/8 step, and the happy-path persistence/index tests are good. I found two issues before treating the schema command surface as stable.

## Findings

- **P1: an indexed field with a real actor id can error after persisting the schema, then make the workspace fail to reopen.** `schema.apply_change` persists `next` and swaps `self.registry` before creating the storage index (`forge/crates/core/src/workspace.rs:636`). But schema field ids interpolate the raw actor id (`f_<actor>_<seq>`, `forge/crates/schema/src/collection.rs:154`), while storage index identifiers reject characters outside `[A-Za-z0-9_./-]` (`forge/crates/storage/src/query.rs:274`). `ActorId` is a transparent string with no such validation (`forge/crates/domain/src/ids.rs:15`), so an indexed `add_field` from an actor like `alice@example.com` mints `f_alice@example.com_0`; `Store::create_index` then returns `QueryError` (`forge/crates/storage/src/index.rs:401`) after the registry has already been written to `__forge/meta`. Because `WorkspaceCore::open` rebuilds indexes from the persisted registry (`forge/crates/core/src/workspace.rs:122`, `forge/crates/core/src/workspace.rs:1197`), the poisoned workspace can fail on every future open/import. Please either make schema-minted field ids valid storage identifiers, reject/normalize unsupported actor ids before applying, or make `schema.apply_change` atomic across registry persistence and index creation.

- **P2: the implemented schema command payloads drift from `forge/spec/commands.md`.** The committed command table still says `schema.apply_change` takes `changes[]` and `schema.validate_compatibility` takes `base_version?, proposed changes[]` (`forge/spec/commands.md:18`), but the implementation/tests accept a singular `{ change }` and `{ against }` registry snapshot (`forge/crates/core/src/workspace.rs:614`, `forge/crates/core/src/workspace.rs:673`, `forge/crates/core/tests/schema.rs:75`, `forge/crates/core/tests/schema.rs:286`). A client following the spec will get a missing-`change` validation error. Please either update the spec/fixtures to the singular snapshot contract or implement the batch `changes[]` / proposed-change validation contract.

## Verification

- `cargo test --locked -p forge-core --test schema`
- `cargo test --locked -p forge-core schema`
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings`
- `git diff --check 591ba035^ 591ba035`

No new handoff file appeared under `task-between-claude-and-codex/` during this wake-up.
