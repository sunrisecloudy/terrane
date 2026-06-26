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
- No sync, server, UI, FFI, native, or policy in the core. Those are *layers*
  added later at the edge, only when a concrete need forces them.

## Layout

- `terrane-core/` — the only product code (Cargo workspace).
  - `crates/terrane-domain/` — pure vocabulary: `Command`, `Event`, `Id`,
    `Error`, `State`. No I/O. Keep it wasm-clean.
  - `crates/terrane-core/` — the engine: `apply(Command) -> [Event] -> State`,
    persistence, replay.
  - `crates/terrane-cli/` — the `terrane` binary (front door).
- `legacy/` — the prior build, kept as reference. Never depend on it.

## Working rules

- Start small and keep it small; add a crate or capability only when forced.
- Keep domain logic deterministic and replayable; effects live at the edge.
- No `unwrap`/panics on real paths — return typed errors.
- Reuse existing terrane-domain types and errors instead of redefining them.
- Commit often: small, green, granular. Branch off `main`. Stage your own files
  explicitly — never `git add -A`. Preserve unrelated dirty/untracked work.
- Mine `legacy/` for hard-won details (CRDT merge, canonicalization,
  conformance cases) and adopt deliberately — copy, don't depend.

## Validation

```sh
cd terrane-core
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -p terrane-cli -- help
```
