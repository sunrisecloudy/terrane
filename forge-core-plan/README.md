# Forge Core Cleanup Plan — Multi-Platform Native Shells

> **Status:** Plan complete, not yet executed. No code has been changed.
> **Goal:** Make Terrane easy to build on many platforms by moving the *same logic that is
> currently reimplemented 5×* into the shared Forge Rust core and into shared **data files**,
> leaving each native shell as a thin layer of genuine OS glue.

This folder is the completed, actionable plan produced from a full audit of `native/` (macOS,
iOS, Windows, Linux, Android) and `forge/` (the Rust core). It is organized so you can read
top-to-bottom for the argument, or jump straight to a phase to execute it.

## The headline

| Measure | Value |
|---|---|
| Native source across 5 shells | **~51K lines** (21.2K Swift + ~30K Kotlin/C/C++/headers) |
| `DevControlPlane` (one DEBUG control surface, reimplemented 5×) | **28,680 lines** (mac 6327 · iOS 3770 · Linux 8267 · Win 8178 · Android 2138) |
| Bridge / policy / storage abstraction layer (reimplemented 5×) | **~12K lines** |
| Duplicated *logic* that should be shared (not OS glue) | **~18K lines** |
| Forge core commands already exposed via the JSON FFI seam | **51** |
| New shared data files proposed (`forge/data/*.json`) | **~12** |

**The control surface has already drifted** from being copied by hand: the same DevControlPlane
exposes **24 routes on macOS, 21 on iOS, 11 on Linux, 10 on Windows, 4 on Android**. Divergence is
already a live correctness problem — the strongest argument for consolidating now.

## The thesis

Three classes of waste, three destinations:

1. **Logic reimplemented per platform** → move the *decision* into the **Forge core** behind the
   existing `terrane_forge_core_handle_command(json) -> json` seam. Only OS glue stays in the shell.
2. **Hard-coded data baked into source** (catalogs, enums, MIME maps, command tables, config) →
   move to **`forge/data/*.json`** that every shell loads once at startup.
3. **Divergent domain state** (a *separate* `apps/app_versions/app_installations` SQLite schema,
   mutated with raw SQL, bypassing the core's `applet.*` lifecycle) → move authority into the
   **core** (storage + new commands), with audit + atomicity + replay.

## How to read this folder

| File | What it covers |
|---|---|
| [00-overview.md](00-overview.md) | Problem statement, goals/non-goals, binding constraints from CLAUDE.md/AGENTS.md |
| [01-findings.md](01-findings.md) | The evidence: quantified duplication table, drift proof, dual-schema split, two concrete bugs |
| [02-target-architecture.md](02-target-architecture.md) | Thin shells + core seam + data files; what crosses into core vs stays per-platform |
| [03-phase-a-data.md](03-phase-a-data.md) | **Phase A** (steps A1–A5): `forge/data/` + extract enums/catalogs/config; shared SQLite schema |
| [04-phase-b-devcontrolplane.md](04-phase-b-devcontrolplane.md) | **Phase B** (B6–B7): consolidate the 28.7K-line DevControlPlane into shared Rust |
| [05-phase-c-security-core.md](05-phase-c-security-core.md) | **Phase C** (C8–C11): network policy, manifest, bridge envelope/permission/budget, recording |
| [06-phase-d-app-lifecycle.md](06-phase-d-app-lifecycle.md) | **Phase D** (D12): core owns app version/rollback/status authority |
| [07-phase-e-crypto.md](07-phase-e-crypto.md) | **Phase E** (E13): unify Ed25519 token/signature seam |
| [08-data-files.md](08-data-files.md) | The `forge/data/*.json` inventory — purpose, schema sketch, source of truth, consumers |
| [09-decisions-and-open-questions.md](09-decisions-and-open-questions.md) | Decisions already made + open questions to answer before late steps |
| [10-validation-and-sequencing.md](10-validation-and-sequencing.md) | Validation gates per phase, build-env constraints, public-contract / Premium pin handling |

## Decisions already locked in

- **Scope:** execute the **full A–E program** (not just data extraction).
- **App lifecycle:** the **Forge core should own** app version history, active-version pointer, and
  status transitions (rollback / quarantine / activation). Shell raw SQL on `app_versions.status`
  becomes illegal. See [09-decisions-and-open-questions.md](09-decisions-and-open-questions.md).

Remaining open questions (control-plane crate location, auto-quarantine policy knobs, canonical
runtime version, public-contract surface, per-platform capability matrix) are listed with
recommended defaults in [09](09-decisions-and-open-questions.md); they only block specific later steps.

## Guiding principles (non-negotiable, from CLAUDE.md / AGENTS.md)

- Domain/business logic is **deterministic & replayable** and lives in the **core**; platform
  effects belong at the **thin shell edge**.
- The public repo owns **app-visible behavior**; preserve it via docs/schemas/fixtures/conformance
  tests. Do **not** add private-SaaS concerns to the public engine.
- Reuse `forge-domain` types and `CoreError`; keep pure-logic crates `wasm32`-clean; no
  `unwrap`/panic on real paths.
- **Every step is a small, green commit.** Branch off `main`, stage own files explicitly, never
  `git add -A`. macOS-first against golden/conformance vectors, then fan out one shell per commit.
