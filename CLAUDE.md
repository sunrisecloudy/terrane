# Claude Notes: Terrane Rust Workspace

## What this is

A deliberate reset of Terrane. New work lives in `rust/` and starts from the
smallest thing that is genuinely the system.

Read `README.md` first — it states the architecture in one diagram.

## The one rule (do not break this)

```
argv ──▶ terrane-host::cli ──▶ Request ──▶ terrane-core ──▶ [Event] ──▶ State (+ persisted log)
```

- The CLI is a thin host adapter. It **never touches data directly** — it only
  builds a request and hands it to the core, then renders the result.
- The core is **deterministic and replayable**: replaying the event log must
  reproduce identical state. Anything that breaks replay-identity is a bug.
- No sync, server, UI, FFI, native, or policy in the core. Those are _layers_
  added later at the edge, only when a concrete need forces them.

## Layout

- `rust/` — the shared Rust crates for local hosts and Premium.
  - `crates/terrane-core/` — shared vocabulary plus the deterministic engine:
    capability registry, `dispatch`→decide→commit→broadcast-fold, persistence,
    replay, and `host_runtime` for `host.run` QuickJS execution.
  - `crates/terrane-cap-*/` — standalone capabilities implementing the common
    `terrane-cap-interface` contract (`app`, `kv`, `crdt`, `harness`, etc.).
  - `crates/terrane-host/` — host services plus the `terrane` binary, C ABI,
    preview store, sync, MCP, and the reusable CLI adapter.
- `host/` — host adapters: `cli/`, `mcp/`, `web/` are Rust packages in the root
  workspace (`terrane-host-cli` etc.); `macos/` is native with its own build.
  Each is a thin client path-depping across the boundary into `rust/crates`.
- `apps/` — app bundles (plain JS, no build system). `apps/todo/` is the first
  app: a `manifest.json` + `main.js` backend over `ctx.resource.kv`.

## Working rules

- Start small and keep it small; add a crate or capability only when forced.
- Keep domain logic deterministic and replayable; effects live at the edge.
- No `unwrap`/panics on real paths — return typed errors.
- Reuse existing terrane-core types and errors instead of redefining them.
- **New commands are new capabilities.** Add a crate under
  `rust/crates/terrane-cap-<name>/` implementing `Capability` (namespace,
  decide, fold, optional describe) and register it in `default_registry`. Never
  reintroduce a central command/event enum or a central decide/fold match.
  Events are name-tagged (`{kind, payload}`); cross-capability reactions go
  through broadcast fold, not direct coupling. **Follow the step-by-step guide
  in `docs/cap-best-practice/`** (start at its README checklist — contract,
  wiring touch points, permissions, tests, release). Capability specs/plans
  live in `plan-completed-cap/`.
- **Tests live in their own files, never inline in the implementation.** Put
  them in the crate's `tests/` directory (integration tests over the public
  surface). The `src/*.rs` files hold code; the proofs live beside them.
- **Host adapters live under `host/` as packages in the root Cargo workspace;
  apps are JS bundles under `apps/`.** A host is a thin client over
  `terrane-host`; it never embeds its own runtime — running app backends is the
  core's `host_runtime`. Apps run their JS backend via `host.run`, which records
  only ordinary `kv.*` events so replay rebuilds without re-running JS (Option
  A).
- **Always run clippy.** After any change, before committing, both must be
  green: `cargo test --workspace --locked` and
  `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Commit often: small, green, granular. Branch off `main`. Stage your own files
  explicitly — never `git add -A`. Preserve unrelated dirty/untracked work.
- When creating or entering a new agent worktree (Codex, Claude Code, or
  OpenCode), copy the canonical Terrane home into that checkout before running
  hosts or app-builder flows:
  `scripts/copy-terrane-home.sh --to "$PWD/.terrane"`. The script defaults to
  `/Users/vehasuwat/Project/terrane/.terrane` when available and skips live
  locks/sockets.

## Validation

Fast local loop — `cargo-nextest` (parallel) plus doctests, with the build cache
sourced:

```sh
scripts/test.sh                      # whole workspace
scripts/test.sh -p terrane-host-web  # one crate (nextest filter args pass through)
```

Before committing, both must be green in the canonical, lockfile-checked form:

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p terrane-host --bin terrane -- help
```

The repo caches builds with a **shared sccache** plus a **per-worktree Cargo
target dir**, so concurrent worktrees never clobber each other's artifacts (a
shared target dir links the wrong rlib across branches). Claude Code project
hooks rewrite Rust build/test Bash calls to source `scripts/cargo-cache-env.sh`
automatically. For manual shells, prefer:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
```

Reclaim target dirs left by deleted worktrees with
`scripts/prune-cargo-targets.sh` (dry-run by default; `--yes` to delete).

Each capability has a file under `tests/cap/` (`tests/cap/main.rs` is the entry
that includes them + shared `helpers`). The engine logic tests live in
`rust/crates/terrane-core/tests/cap/`; the real binary-level e2e tests in
`rust/crates/terrane-host/tests/cap/`. The effectful e2e (`net`, `model`) hit
the real network / real agent CLIs, so they are `#[ignore]`d — keep the default
`cargo test` green and run them deliberately:

```sh
cargo test -p terrane-host -- --ignored   # real fetch + real agent call
```

Add an e2e test for each new capability (pure ones run by default; effectful
ones `#[ignore]` with a reason).

Validate touched host adapters with package-scoped commands when a full
workspace run is more than the change needs:

```sh
cargo test -p terrane-host-cli --locked
cargo clippy -p terrane-host-cli --all-targets --locked -- -D warnings
```
