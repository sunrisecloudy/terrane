---
name: new-capability
description: Best practices and step-by-step workflow for adding a new Terrane capability crate (terrane-cap-NAME) to the rust/ workspace. Use when asked to add a new capability, add a new command/namespace, extend a capability's surface (commands, events, resources, effects, grants), or review a capability implementation for contract violations. Routes to the canonical guide in docs/cap-best-practice/ and lists the wiring touch points, drift guards, and validation gate.
---

# New Terrane Capability

A capability is a self-contained slice of engine behaviour owning one
namespace: its commands, events, queries, `ctx.resource` methods, grant specs,
deciding, and folding. There is no central command/event enum — you add a
command by writing and registering one capability.

**The canonical guide is `docs/cap-best-practice/` — do not rediscover the
contract from raw code.** Read `docs/cap-best-practice/README.md` first (TL;DR
checklist + reference-crate table), then the numbered doc for the step you are
on:

| Doc | Covers |
| --- | --- |
| `01-design.md` | Is it a capability? Shape (pure / effectful / runtime / event-only / resource-only / projection), naming |
| `02-contract.md` | decide/fold split, broadcast fold, replay identity, typed errors, registry validation |
| `03-skeleton-and-wiring.md` | Copy `terrane-cap-net`, rename; the four touch points (workspace members, core dep, `State` field + `get`/`get_mut` arms, `default_registry()`) |
| `04-cross-capability.md` | Bus queries, subscriptions (`app.removed` cleanup!), reserved-KV projections |
| `05-effects-and-runtimes.md` | `Effect` variant + edge runner arm, cap-owned event constructors, run-once-record-result |
| `06-permissions-and-policy.md` | Resource methods + `namespace_v1` grant spec, classify commands in `public_authz.rs`, bypass audit |
| `07-testing.md` | Four test layers: `src/tests.rs`, `tests/capability.rs`, engine tests with `replay_matches()`, binary e2e |
| `08-public-surface-and-release.md` | CLI/MCP/HTTP exposure, contract export, old-log replayability |
| `09-docs-and-done.md` | `doc()`/`describe()`, regenerate `APP_API.md`, i18n strings in public KV, full gate |
| `10-case-study-hybrid-search.md` | Worked end-to-end example |

Reference crates (read code before writing code): `terrane-cap-net` — minimal
complete capability + effect pattern; `terrane-cap-time` — smallest
recorded-vs-transient effect pair with a per-run call cap; `terrane-cap-kv` —
mature module split, storage backends, `app.removed` cascade.

## Non-negotiables

- Replay identity: `decide` is pure; nondeterminism goes through an `Effect`
  recorded as a fact; `fold` rebuilds state from records only. App JS runs
  once, never on replay.
- Typed `terrane_cap_interface::Error` — no `unwrap`/panics on real paths.
- Shipped event payload shapes are frozen; evolve by adding new kinds.
- Cross-capability: react via broadcast-fold subscriptions, read via
  `ctx.bus.query` — never a crate dep on another capability.
- `Registry::validate()` refuses: names not prefixed `namespace.`, duplicate
  names, subscriptions to undeclared events, resource methods without a
  `namespace.v1` grant spec covering every method kind (`read`/`write`/`call`).

## Gotchas that cost time

- E2e flows reach `ctx.resource.<ns>` only if the app manifest lists the
  namespace in `resources` AND a grant exists:
  `terrane auth grant user:local-owner <app> <ns>`.
- Changed `resource_api`? A drift test fails until you regenerate:
  `UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc`.
- New commands must be classified in `public_authz.rs` (an authz reconcile
  test counts them) — see `06-permissions-and-policy.md`.
- Engine/binary test files need a `mod <name>;` line in the respective
  `tests/cap/main.rs`; effectful binary e2e is `#[ignore = "reason"]` so the
  default run stays green (`cargo test -p terrane-host -- --ignored` to run).

## Gate before every commit

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p terrane-host --bin terrane -- help
```

Fast loop: `scripts/test.sh` (nextest + doctests, cached). In agent worktrees
wrap manual cargo calls with `scripts/with-cargo-cache.sh`. Commit small,
green, granular; stage your own files explicitly — never `git add -A`.
