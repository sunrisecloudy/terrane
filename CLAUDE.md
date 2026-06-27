# Claude Notes: terrane-core

## What this is

A deliberate reset of Terrane. The previous build (a 17-crate Rust workspace
plus native hosts, runtime-web, servers, and specs) was swept intact into
`legacy/` and is now **reference only**. New work lives in `terrane-core/` and
starts from the smallest thing that is genuinely the system.

Read `README.md` first — it states the architecture in one diagram.

## The one rule (do not break this)

```
argv ──▶ terrane-cli ──▶ Command ──▶ terrane-core ──▶ [Event] ──▶ State (+ persisted log)
```

- The CLI is a thin arg parser. It **never touches data directly** — it only
  builds a Command and hands it to the core, then renders the result.
- The core is **deterministic and replayable**: replaying the event log must
  reproduce identical state. Anything that breaks replay-identity is a bug.
- No sync, server, UI, FFI, native, or policy in the core. Those are _layers_
  added later at the edge, only when a concrete need forces them.

## Layout

- `terrane-core/` — the engine + base CLI (Cargo **workspace A**).
  - `crates/terrane-domain/` — shared vocabulary: `Request`, `EventRecord`,
    `Error`, ids. Pure, no I/O.
  - `crates/terrane-core/` — the deterministic engine: capability registry,
    `dispatch`→decide→commit→broadcast-fold, persistence, replay. `src/cap/*`
    are the capabilities (`app`, `kv`, `net`, `model`, `host`). The `host`
    capability runs app backends in **QuickJS** (`rquickjs`) over a sandboxed,
    app-scoped `ctx.resource` — QuickJS lives ONLY here, never in a host.
  - `crates/terrane-cli/` — the `terrane` binary plus a reusable **lib** spine
    (`run`/`dispatch`/`open`/`EdgeRunner`) that hosts wrap.
- `host/` — hosts, each its **own Cargo workspace** (separate build) so non-Rust
  hosts aren't entangled. `host/cli/` is `terrane-host`, the first host; it
  path-deps across the boundary into `terrane-core/crates` and adds `run <app>`.
- `apps/` — app bundles (plain JS, no build system). `apps/todo/` is the first
  app: a `manifest.json` + `main.js` backend over `ctx.resource.kv`.
- `legacy/` — the prior build, kept as reference. Never depend on it.

## Working rules

- Start small and keep it small; add a crate or capability only when forced.
- Keep domain logic deterministic and replayable; effects live at the edge.
- No `unwrap`/panics on real paths — return typed errors.
- Reuse existing terrane-domain types and errors instead of redefining them.
- **New commands are new capabilities.** Add a module under
  `terrane-core/src/cap/` implementing `Capability` (namespace, decide, fold,
  optional describe) and register it in `default_registry`. Never reintroduce a
  central command/event enum or a central decide/fold match. Events are
  name-tagged (`{kind, payload}`); cross-capability reactions go through
  broadcast fold, not direct coupling.
- **Tests live in their own files, never inline in the implementation.** Put
  them in the crate's `tests/` directory (integration tests over the public
  surface). The `src/*.rs` files hold code; the proofs live beside them.
- **Hosts are separate workspaces under `host/`; apps are JS bundles under
  `apps/`.** A host is a thin client over the `terrane_cli` spine; it never
  embeds its own runtime — running app backends is the core's `host` capability.
  Apps run their JS backend via `host.run`, which records only ordinary `kv.*`
  events so replay rebuilds without re-running JS (Option A).
- **Always run clippy.** After any change, before committing, both must be
  green: `cargo test` and `cargo clippy --all-targets -- -D warnings`.
- Commit often: small, green, granular. Branch off `main`. Stage your own files
  explicitly — never `git add -A`. Preserve unrelated dirty/untracked work.
- Mine `legacy/` for hard-won details (CRDT merge, canonicalization, conformance
  cases) and adopt deliberately — copy, don't depend.

## Validation

```sh
cd terrane-core
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -p terrane-cli -- help
```

Tests mirror `src/cap/`: each capability has a file under `tests/cap/`
(`tests/cap/main.rs` is the entry that includes them + shared `helpers`). The
engine logic tests live in `terrane-core/tests/cap/`; the real binary-level e2e
tests in `terrane-cli/tests/cap/`. The effectful e2e (`net`, `model`) hit the
real network / real agent CLIs, so they are `#[ignore]`d — keep the default
`cargo test` green and run them deliberately:

```sh
cargo test -p terrane-cli -- --ignored   # real fetch + real agent call
```

Add an e2e test for each new capability (pure ones run by default; effectful
ones `#[ignore]` with a reason).

Each host is its own workspace — validate it separately:

```sh
cd host/cli && cargo test && cargo clippy --all-targets -- -D warnings
```
