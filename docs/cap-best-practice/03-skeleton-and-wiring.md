# 03 — Crate skeleton and engine wiring

The fastest correct start: copy `rust/crates/terrane-cap-net/` (a complete
capability in ~140 lines) and rename. This page is what you're copying.

## Cargo.toml

```toml
[package]
name = "terrane-cap-<name>"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
name = "terrane_cap_<name>"
path = "src/lib.rs"

[dependencies]
terrane-cap-interface.workspace = true
borsh.workspace = true
```

Rules:

- Depend on `terrane-cap-interface` — **never on `terrane-core`** (the core
  depends on you, not the reverse).
- Depend on another cap crate only for a sanctioned pattern: the reserved-KV
  projection uses `terrane-cap-kv`'s exported helpers
  ([04-cross-capability.md](04-cross-capability.md)).
- borsh is the event-log format. For JSON at the edges (manifests, selector
  schemas, doc payloads) use `serde_json` or `nanoserde` from the workspace —
  **never hand-build JSON with `format!`/`Debug`** (a past review caught this;
  it breaks on the first quote or backslash in input).

## File layout

Minimum viable crate:

```
terrane-cap-<name>/
├── Cargo.toml
└── src/
    ├── lib.rs      # Capability impl: namespace, manifest, decide, fold, describe
    ├── doc.rs      # the CapabilityDoc for `cap info` / MCP
    └── tests.rs    # unit tests, included via `#[cfg(test)] mod tests;`
```

Grow module-by-module when `lib.rs` gets crowded — `terrane-cap-kv` shows the
mature split: `commands.rs` (decide fns), `events.rs` (fold + describe),
`types.rs` (state + payload structs), `resources.rs` (the `ctx.resource`
surface), `storage.rs` (domain logic). Add `tests/capability.rs` for
integration tests over the public trait surface ([07-testing.md](07-testing.md)).

## Minimal lib.rs shape

```rust
//! The `<ns>` capability — one sentence on what fact it owns.

use terrane_cap_interface::{ /* … */ };

mod doc;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct <Name>State { /* BTreeMaps of your facts */ }

#[derive(BorshSerialize, BorshDeserialize)]
struct SomethingHappened { /* event payload */ }

pub struct <Name>Capability;

impl Capability for <Name>Capability {
    fn namespace(&self) -> &'static str { "<ns>" }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec { name: "<ns>.verb" }],
            events: vec![EventSpec { kind: "<ns>.happened" }],
            queries: Vec::new(),
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern { kind: "app.removed" }],
        }
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc { doc::<ns>_doc(include_internal) }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "<ns>.verb" => { /* validate args, then Commit / Effect / Runtime */ }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "<ns>.happened" => { /* decode_event + state_mut */ }
            "app.removed" => { /* drop that app's slice */ }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> { /* one line per own event */ }
}

#[cfg(test)]
mod tests;
```

## Wiring: the four touch points outside your crate

1. **`Cargo.toml`** — add the crate to `[workspace] members` and to
   `[workspace.dependencies]`.
2. **`rust/crates/terrane-core/Cargo.toml`** — add
   `terrane-cap-<name>.workspace = true`.
3. **`State`** (`rust/crates/terrane-core/src/lib.rs`) — if you have a state
   slice: add the field, plus a match arm in **both** `StateStore::get` and
   `StateStore::get_mut` for your namespace. Miss one and `state_ref`/
   `state_mut` returns a runtime error, not a compile error.
4. **`default_registry()`** (same file) — `registry.register(Box::new(…))`.
   Registration `expect`s a unique namespace and the registry then
   `validate()`s every manifest ([02-contract.md](02-contract.md)), so
   declaration mistakes fail at first `Core::open` in any test.

Effectful shapes have a fifth touch point (the host's `EdgeRunner`) and
resource shapes a policy touch point — see
[05-effects-and-runtimes.md](05-effects-and-runtimes.md) and
[06-permissions-and-policy.md](06-permissions-and-policy.md).

Next: [04-cross-capability.md](04-cross-capability.md) — talking to other capabilities without coupling.
