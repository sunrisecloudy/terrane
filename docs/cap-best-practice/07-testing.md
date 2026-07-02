# 07 — Testing

A capability is proven at four layers. Code lives in `src/*.rs`; proofs live
beside it in their own files — **never inline `mod tests { … }` blocks inside
implementation files** (a past review moved `auth`'s out; don't repeat it).

| Layer | Where | Drives | Proves |
|---|---|---|---|
| Unit | `src/tests.rs` via `#[cfg(test)] mod tests;` in `lib.rs` | Internal fns | Parsing, validation, edge cases |
| Capability | `tests/capability.rs` in your crate | `decide`/`fold`/`read_resource` directly, with stub `StateStore` + `CapBus` | The trait surface in isolation |
| Engine | `rust/crates/terrane-core/tests/cap/<ns>.rs` | `Core::dispatch` | Events, state, errors, **replay identity**, cascades |
| Binary e2e | `rust/crates/terrane-host/tests/cap/<ns>.rs` | The real `terrane` binary | The whole front door |

`terrane-cap-kv` has all four; use it as the template.

## Engine tests (the load-bearing layer)

Add `mod <ns>;` to `rust/crates/terrane-core/tests/cap/main.rs` and use the
shared fixtures in `helpers.rs` — `req` (trusted), `public_req`,
`grant_resource`:

```rust
let dir = tempdir().unwrap();
let mut core = Core::open(&dir.path().join("log.bin")).unwrap();
core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

core.dispatch(req("<ns>.verb", &["notes", "…"])).unwrap();
assert_eq!(core.state().<ns>…, …);
assert!(core.replay_matches().unwrap());
```

Non-negotiables for every engine test file:

- **Assert `core.replay_matches().unwrap()` after mutations.** This is the
  replay-identity contract; a capability without this assertion isn't tested.
- Re-open the log (`Core::open(&log)`) at least once and assert the rebuilt
  state — proves cold-start replay, not just the in-memory fold
  (`kv_records_and_cascades_via_broadcast_fold` does both).
- Test the error paths as values: `assert_eq!(core.dispatch(…),
  Err(Error::AppNotFound("ghost".into())))`.
- If you subscribed to `app.removed`, test the cascade: write, remove the app,
  assert your slice is empty.

Registry-wide inventory tests also live here — if you declared resources, add
your namespace to `grant_spec_inventory.rs` / `grant_verbs_match_specs.rs`
([06-permissions-and-policy.md](06-permissions-and-policy.md)), and expect the
`docs/APP_API.md` drift test in `host.rs` to demand a regeneration.

## Policy and client-surface tests

Capabilities with commands or resources also need host-policy coverage:

- `rust/crates/terrane-host/tests/public_authz.rs` — classify every registered
  command/query; assert public commands either allow, grant-gate with a stable
  app arg index, or refuse with a reason.
- `rust/crates/terrane-host/src/mcp_tests.rs` — when MCP behavior changes, cover
  `capability_command` help/dry-run/dispatch, `permission_required` shape, and
  refusal text visible to agents.
- `rust/crates/terrane-host/tests/contract.rs` — when exported surface changes,
  verify the machine-readable contract still matches live declarations.

## Binary e2e tests

Add `mod <ns>;` to `rust/crates/terrane-host/tests/cap/main.rs`. The helper
spawns the built binary against a throwaway home:

```rust
let (ok, out, err) = terrane(home, &["<ns>", "verb", "notes", "…"]);
assert!(ok, "stderr: {err}");
```

- Pure capabilities: a small smoke test that runs by default. Logic detail
  belongs in the engine layer, not here.
- Effectful capabilities: mark the real-I/O test
  `#[ignore = "real network fetch; run with `cargo test -- --ignored`"]`
  (see `net.rs`, `model.rs`), guard external CLIs with `helpers::on_path`, and
  keep the reason string honest — it's the operator's documentation.
- Every new capability gets an e2e test: pure ones default-run, effectful ones
  `#[ignore]`d with a reason.

## The gate

Green before every commit, from `rust/`:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Run the effectful suite deliberately when you touched it:

```sh
cargo test -p terrane-host -- --ignored
```

And validate the separate host workspace if you touched the CLI surface:

```sh
cd host/cli && cargo test && cargo clippy --all-targets -- -D warnings
```

Next: [08-public-surface-and-release.md](08-public-surface-and-release.md).
