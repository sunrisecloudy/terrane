# PRD 01 — Core Runtime (`forge-core`)

**Status:** Merged draft v1 · **Depends on:** — · **Depended on by:** all
**Sources:** F-01 (engine trait, capabilities, limits, lifecycle) + P-04 (crate layout, command/event API, error model) + P-07 (deterministic mode, entrypoint contract, ctx API) + decisions D1, D4, D7

## 1. Purpose

A single Rust workspace providing the execution environment for user/LLM code: JS engine hosting, capability sandbox, resource governance, applet lifecycle, deterministic run recording, and the versioned command/event API consumed by every platform shell — **and by the M0 CLI harness, which is a shell like any other**.

## 2. Crate layout (adopted from P-04)

```
crates/
  core/        public command/event/stream facade (the shell contract)
  domain/      domain types and validation
  storage/     SQLite KV/oplog/index layer (PRD 02)
  schema/      dynamic relational schema/query engine (PRD 02)
  crdt/        Loro wrappers and document mapping (PRD 02)
  sync/        client + server sync protocol (PRD 03)
  runtime/     JsEngine trait, QuickJS + JSC impls, run manager
  policy/      RBAC and capability engine (PRD 07)
  llm/         provider abstraction and pipeline (PRD 04)
  ui/          component-tree model, diffing, patch protocol (PRD 05)
  secrets/     keychain/keystore abstraction
  audit/       logs, retention, redaction
  ffi/         UniFFI / C-ABI / wasm-bindgen exports — generated, thin
  server/      forge-server: cloud + embedded deployments (PRD 03)
  testkit/     deterministic fixtures, conformance suites, harness lib
  cli/         the M0 harness / future SDK CLI
```

## 3. Shell contract: Command / Event / Stream (adopted from P-04)

- **CR-A1** All shells call the core through versioned `Command<Request,Response>`, subscribe to `Event<Payload>`, and attach to `Stream<Payload>` (UI tree patches, logs, sync status). No shell may mutate SQLite, CRDT docs, permissions, schemas, or runtime state directly — enforced by the binding layer exposing no such path.
- **CR-A2** Command catalog (initial; full list in `/spec/commands.md`): `workspace.create/open/export/import`, `applet.install/upgrade/suspend/uninstall`, `file.write/history/restore_version`, `schema.apply_change/validate_compatibility/rebuild_indexes`, `query.execute`, `record.put/patch/delete/hard_purge`, `runtime.run/cancel/replay/get_logs`, `ai.generate_patch/apply_patch/run_fix_loop/set_context_mode`, `sync.start/stop/status/invite/accept_invite`, `permission.request_grant/revoke`, `rbac.create_role/assign_role`, `secret.store/revoke`.
- **CR-A3** Every command carries `ActorContext` and passes RBAC/capability validation in `policy` before touching state (P-04 boundary rule; PRD 07).
- **CR-A4** Errors are typed, stable, user-displayable, machine-actionable: `ValidationError, PermissionDenied, CapabilityRequired, StorageError, SchemaCompatibilityError, QueryError, RuntimeError, ResourceLimitExceeded, SyncError, ConflictRequiresUser, ProviderError, PlatformUnavailable`. FFI calls never panic across the boundary (F:CR-13).
- **CR-A5** Commands are versioned; unknown persisted event fields are preserved; older clients reject unsupported commands gracefully with capability negotiation (P-04).

## 4. Sandbox requirements

- **CR-1** All applet/script code executes inside an embedded JS engine with **zero ambient capabilities**: no filesystem, network, clock, randomness, or host memory access except through explicitly injected host functions.
- **CR-2** JS engine is pluggable behind a Rust `JsEngine` trait. **Decision D4 (amended by Review 001): QuickJS is the non-negotiable spine; JSC is co-resident within M0** — the M0a spine proof runs on **QuickJS** (`rquickjs` native; QuickJS-WASM in web workers), and **JavaScriptCore** (Apple platforms: macOS first shell, iOS later; App Store 2.5.2-sanctioned path + JIT) lands in M0b together with the conformance suite (CR-12), which then runs its covered vectors on every wired engine in CI.
- **CR-3** Host API ("syscalls") is capability-scoped, exposed to code as `ctx` with generated type definitions. Namespaces: `db` (query/mutate granted collections), `storage` (per-applet KV), `ui` (tree updates, PRD 05), `net` (`ctx.http.fetch` to manifest-allowlisted domains only), `llm` (budgeted completions/objects/embeddings), `schedule` (timers, background tasks), `secrets` (write-only references), `files` (user-granted handles only), `time`/`random` (deterministic seams), platform capabilities (`clipboard, notifications, camera, microphone, location, contacts, calendar, email`) returning `PlatformUnavailable` where unsupported (P-07).
- **CR-4** Every host call is checked against the granted manifest **at call time**; revocation takes effect immediately.
- **CR-5** Resource limits per instance (defaults, shell-configurable): memory 64 MB; CPU via engine interrupt every 10 ms with 100 ms budget per event-loop turn; 50 pending timers; 5 concurrent net requests; plus per-run manifest limits `cpu_ms, wall_ms, memory_mb, network_bytes, storage_bytes, log_bytes, output_bytes` (P-07). Exceeding limits → suspension with user-visible error, never host crash. Fuel/limit accounting lives in the **shared host shim**, not per-engine, so semantics match across JSC and QuickJS.
- **CR-6** Event-loop model: applets are event-driven (UI events, `db.watch` notifications, timers, sync events). No applet-owned threads; one realm per applet; a work-stealing pool services realms on native, one Worker per N applets on web.

## 5. Two runnable shapes (merged product model)

- **CR-7 Applet** (F): long-lived, UI-bearing, event-driven; renders via `ctx.ui.render(tree)`; lifecycle `install → enable → run → suspend → upgrade → uninstall`; upgrade is atomic (code + schema additions in one transaction or rollback); uninstall offers data retention choice.
- **CR-8 Script/automation** (P): run-to-completion unit with `export async function main(ctx, input): Promise<Result>`, a manifest (`entrypoint, deterministic, capabilities, limits`), and optional **deterministic mode**: `ctx.time.now()` and `ctx.random` return recorded/seeded values; live network is forbidden unless the run is marked non-deterministic or replays a recorded fixture; all host responses are recorded; `runtime.replay` reproduces the run byte-identically on any platform. Applets may also declare deterministic event handlers for testability.
- **CR-9** Run records (P-04): every execution persists code hash, input, permission snapshot, host API responses, logs, resource usage, and resulting writes — powering replay, debugging, and audit.

## 6. Code shape & compatibility

- **CR-10** Applet package = CRDT documents (PRD 02): `manifest`, `src/*.ts` (multi-file, local imports only, collaborative text CRDTs), `tests/*.ts`. Code is data; it syncs and has history like everything else.
- **CR-11** Host API is versioned (`forge-api@MAJOR.MINOR`); applets declare `minApi`; unknown future host functions are absent (feature-detect via `forge.has(...)`), never erroring stubs; no capability removal within a MAJOR.
- **CR-12** **Cross-engine conformance suite** (release-blocking for covered vectors): the M0b corpus pins byte-identical `main(ctx,input)` behavior for JS-language divergence areas, deterministic seams, and run/replay fingerprints through the engine-agnostic `JsEngine` harness. It does **not** currently claim complete coverage of every host API, UI event-dispatch path, live-query notification path, or resource-limit mode; those vectors must be added before that broader claim becomes a release gate. Built on top of the M0a spine, this is the heart of the platform template. The central acceptance test of the whole product remains the spine itself: *TS → SWC → QuickJS-WASM → Rust capability `ctx` → SQLite write → UI tree patch → deterministic replay, fully offline.*
- **CR-13** Strict TS profile; no `eval`, `Function`, dynamic `import`, prototype pollution of the host bridge — disabled at engine level **and** rejected by the static policy scan (PRD 04 LM-9): two independent layers.

## 7. TS toolchain placement (Decision D7: fully offline on every platform)

- **CR-14** Transpile: **SWC** in-core (Rust), always offline, < 5 ms typical applet.
- **CR-15** Full type-check, offline everywhere: desktop/server run a managed native TS compiler (`tsgo`/TS7-class) sidecar, version-pinned to the core release, supervised by the shell; **web** runs the TypeScript compiler in a **lazy-loaded optional worker artifact — explicitly outside the initial core size budget (§8)**, fetched on first edit/generate and cached for offline use thereafter (read-only/viewer sessions never pay for it); **mobile** bundles the same checker hosted in the local engine. No cloud dependency exists anywhere in the pipeline. Type-check latency and the checker artifact size are tracked perf gates (PRD 09). Until the checker is cached, web falls back to transpile + policy scan with an honest "type-check pending download" state — it never silently skips verification for an install.
- **CR-16** Applets ship as type-checked, transpiled JS + source maps; sources retained for editing/regeneration.

## 8. Performance budgets

Applet cold start (install → first UI frame): < 150 ms desktop, < 400 ms web. Warm UI-bound event handling: < 16 ms p95 desktop. Host-call overhead: p95 < 50 µs native, < 200 µs web. Core binary: < 12 MB native, < 6 MB WASM gzipped — **excluding the lazy type-checker worker artifact (CR-15), which is budgeted separately (target ≤ 10 MB gzipped, lazy + cached)**. Tracked in CI with hard gates.

## 9. Out of scope (v1)

WASM applets; multi-threaded applets; npm imports; native plugin ABI; shared mutable state between applets (communicate via mutually-granted collections only).

## 10. Acceptance

- M0: full loop (install → run → store → sync → UI tree → event → patch) green headlessly via the CLI harness on macOS/Linux/WASM CI targets.
- Covered CR-12 engine vectors green on every wired engine target; any divergence in the covered corpus blocks release, and new host/API/limit/UI vectors become blocking as they are promoted into the suite.
- Hostile-applet corpus (infinite loops, alloc bombs, recursion bombs, host-call floods, forbidden globals, prototype pollution) → 100% contained; kill/revoke from shell < 100 ms.
- Deterministic replay: same script + same recorded responses → identical output on every platform.
