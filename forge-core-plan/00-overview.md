# 00 — Overview

## Problem statement

Terrane ships native shells for five platforms: macOS (Swift), iOS (Swift), Windows (C++),
Linux (C), Android (Kotlin). Each shell was built largely by **reimplementing the same logic in a
different language**. The result:

- Adding or changing a behavior means editing it in **five places**, in four languages.
- The copies have already **drifted** out of sync (see [01-findings.md](01-findings.md)).
- A large amount of **domain logic** (app version management, rollback, quarantine, network policy,
  manifest parsing, bridge permission checks) lives in the shells, where CLAUDE.md says it must
  **not** — it belongs in the deterministic, replayable Forge core.
- A lot of **information is hard-coded in source** (app catalogs, command tables, enums, MIME maps,
  config constants) rather than living as data that all shells share.

The Forge Rust core already exists and already exposes a clean seam — a single JSON command
interface (`terrane_forge_core_handle_command(json) -> json`) with **51 commands** and an event
stream. The shells under-use it: instead of delegating, they re-derive logic locally.

## Goals

1. **Reuse, don't reimplement.** Move every piece of *decision logic* that is identical across
   platforms into the Forge core (behind the existing JSON seam) or into a small set of new pure
   Rust modules exposed through that seam.
2. **Information as data.** Replace hard-coded catalogs/enums/tables/config in Swift/Kotlin/C/C++
   with shared `forge/data/*.json` files loaded once per shell.
3. **One source of truth for persistence.** Collapse the shell-owned `apps/app_versions/
   app_installations` SQLite schema into the core's storage + new lifecycle commands.
4. **Thin shells.** Leave each shell as only what is genuinely platform-specific: HTTP listeners,
   WebView + dialogs, SQLite FFI transport, key custody, OS install/uninstall.
5. **Make a 6th platform cheap.** After this work, porting becomes "implement the glue + load the
   data," not "re-derive 18K lines of logic."

## Non-goals

- **Not** rewriting the per-platform OS integration (WKWebView/NWListener/Soup/WinHTTP/Room). That
  glue is legitimately different per platform and stays.
- **Not** adding private-SaaS concerns (identity, billing, hosted sync, marketplace custody) to the
  public engine. Those live in `../terrane-premium` and consume the public contract.
- **Not** changing app-visible behavior silently. Where behavior is app-visible, it is preserved by
  docs/schemas/fixtures/conformance tests and the public-contract export.
- **Not** a "big bang" rewrite. Every step is independently shippable and green.

## Binding constraints (from `CLAUDE.md`, `AGENTS.md`)

- **Determinism & replay:** business logic must be deterministic and replayable, and live in the
  core. Platform effects belong at the shell edge. Replay-sensitive changes are gated by the
  replay-identical check.
- **Public-contract ownership:** when a behavior can be observed by generated apps/applets,
  implement and document it **here** first, and surface it through `artifacts/public-contract.json`.
  Premium consumes the contract; this repo must not depend on Premium.
- **Reuse domain types:** use `forge-domain` (`CoreError`, ids, `Manifest`, `RunRecord`, …); do not
  redefine them. Keep pure-logic crates (`domain`, `schema`, `policy`, `ui`, pipeline core)
  `wasm32`-clean; native-only deps stay behind `cfg(not(target_arch = "wasm32"))`.
- **Error discipline:** return `CoreError`; no panic/`unwrap` on real paths (tests may `unwrap`).
- **Commit hygiene:** never `git add -A`; never commit `forge/target/`; small green commits; branch
  off `main`, stage own files explicitly.

## The shape of the fix (one picture)

```
  Today (per platform, ×5)                     Target
  ┌───────────────────────────┐                ┌───────────────────────────┐
  │ Shell (Swift/Kt/C/C++)    │                │ Shell (thin OS glue only) │
  │  • WebView / dialogs      │                │  • WebView / dialogs      │
  │  • HTTP listener          │                │  • HTTP listener          │
  │  • SQLite transport       │                │  • SQLite transport       │
  │  • app registry + rollback│  ── move ──▶   │  • key custody            │
  │  • network policy / IP    │                │  • OS install/uninstall   │
  │  • bridge perms / budget  │                └─────────────┬─────────────┘
  │  • manifest parsing       │                              │ JSON seam (51+ cmds)
  │  • snapshot/html/a11y/…   │                ┌─────────────▼─────────────┐
  │  • hard-coded catalogs    │                │ Forge core (Rust)         │
  └───────────────────────────┘                │  + new commands           │
                                               │  + forge-controlcore      │
                                               └─────────────┬─────────────┘
                                                             │ loads
                                               ┌─────────────▼─────────────┐
                                               │ forge/data/*.json (shared)│
                                               └───────────────────────────┘
```

See [02-target-architecture.md](02-target-architecture.md) for the detailed seam and the
stays-per-platform vs moves-to-core split.
