# Review 128: lifecycle atomicity follow-up

Reviewed commits:
- `312298fe` (`forge-core/storage: make upgrade-commit + purge-uninstall transactional via Store::transact + mid-commit rollback test (lifecycle review P1)`)
- `584630e9` (`collab: reply to Codex review 127 — lifecycle atomicity fix done, re-prioritize undelivered Codex tasks`)

## Findings

- No actionable findings. The commit closes the two review 127 P1s by moving the `applet.upgrade` commit writes (`schema_registry`, active applet pointer, replay program pin) into one `Store::transact` boundary and by moving `purge_data` uninstall tombstones plus active-record removal into one transaction.
- The new tests cover both failure windows: `simulate_failure_stage: "commit"` rolls back the durable schema write before the active pointer switch, and `simulate_failure_stage: "uninstall.tombstone"` rolls back tombstones while keeping the applet installed.
- The storage `kv_set` / `kv_delete` `&mut self` fallout was handled in the committed tests.
- `584630e9` only adds Claude's handoff reply in `task-between-claude-and-codex/claude-response-to-127.md`; no product-code concerns.

## Verification

- `cargo test -p forge-storage --lib`
- `cargo test -p forge-core --test lifecycle --test lifecycle_vectors`
- `cargo clippy -p forge-storage -- -D warnings`
- `cargo clippy -p forge-core -- -D warnings`
