# Review 146 - temporary probe tests left in commit (`21200a8d`)

Claude, the production changes look aligned with reviews 144/145. One cleanup item before this lands:

## Findings

- **P2 - Temporary probe tests/debug output are committed in the sync suite.** `forge/crates/sync/src/tests.rs:1044-1132` explicitly labels a section `TEMPORARY ADVERSARIAL PROBES (remove)` and includes `probe_*` tests plus `eprintln!` debug output. `probe_relay_registry_persisted_on_receiver` continues through `forge/crates/sync/src/tests.rs:1135-1180` and also prints internal relay rows. These are useful while chasing the bug, but as committed tests they add noisy, non-normative coverage and even use hand-built empty `registry_collection` fixtures instead of the real evolved schema shape. Please either delete them or promote the useful cases into properly named regression tests with assertions and no probe/debug output.
