# Review 026 - 21bf1d22 runtime replay fallback

Commit reviewed: `21bf1d221af2f22678e86e322e0fa0cc0dbf7114`

## Finding

- **P1 - Stripping `permissions` can turn a recorded denial into a successful replay.** The legacy path treats any record whose `permissions == PermissionSnapshot::default()` as snapshotless and rebuilds replay policy from the current manifest/actor (`forge/crates/runtime/src/runner.rs:129`). That loses the difference between an actually old record and a tampered/new record with the `permissions` field removed. For a post-CR-9 failed run that recorded a denied `storage.set`, removing the `permissions` field and replaying under a manifest that now grants the write makes `check_or_record_denial()` pass (`forge/crates/runtime/src/host.rs:111`), then `storage_set()` consumes the recorded `{"denied": ...}` response through `host_call()` but ignores the response and returns `Ok(())` (`forge/crates/runtime/src/host.rs:171`). The replayed `RunRecord` can therefore complete even though the original failed, unless every caller remembers to compare fingerprints afterward. Please make legacy fallback distinguish true absence from an explicit/default snapshot (for example via a migration/deserialization marker), or at minimum refuse the fallback for records with denied responses / failed outcomes, and add a regression test that removes `permissions` from a recorded denial and asserts replay still fails.

## Notes

- `git show --check 21bf1d22` passed.
- `cargo test --locked -p forge-runtime --test containment` passed: 11 tests.
- `cargo test --locked -p forge-runtime --test determinism` passed: 12 tests.
- `cargo test --locked -p forge-runtime --lib --no-run` passed.
