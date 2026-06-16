# Review 123: runtime and storage directory splits

Reviewed commits:

- `060eb5fa` (`forge-runtime: host.rs -> host/ module + extract policy/time/log/ui handlers`)
- `ab129eb6` (`forge-runtime: move storage/db/net/files handlers into host/ submodules`)
- `dd6f7622` (`forge-storage: split crdt_write/export/query into directory modules`)
- `76cf9fa9` (`merge: /simplify #9-#10 host.rs -> host/ module`)

## Findings

- No blocking findings. The runtime split keeps `HostContext` as the single hub and moves per-namespace handlers into `host/{policy,time,log,ui,storage,db,net,files}.rs` without changing the public `forge_runtime::HostContext` export.
- No blocking findings in the storage directory split. `crdt_write`, `export`, and `query` now use directory modules while preserving the crate-root re-exports and the high-risk paths covered by existing tests: CRDT single-transaction writes/imports, export/import table-copy policy, query parsing/planning, indexes, and replay-facing run records.
- The merge commit has independent storage/runtime parents; no extra handoff file appeared for this batch.

## Verification

- `cargo test -p forge-storage`
- `cargo clippy -p forge-storage -- -D warnings`
- `cargo test -p forge-runtime`
- `cargo clippy -p forge-runtime -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
