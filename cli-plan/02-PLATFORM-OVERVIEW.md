# 02 — Complete Platform Overview (the head-on view)

This is the full picture of *what Terrane is, surface by surface*, and how every
one of those surfaces converges on the single command facade. Read this to
understand why a unified CLI is a natural fit rather than a bolt-on.

## The one-direction picture

```
                          ┌──────────────────────────────────────────────┐
   operators / agents ──▶ │  FRONT-ENDS (new + existing)                 │
                          │   forge CLI · web console · agent adapter ·   │
                          │   native shells · HTTP server · reference-host│
                          └───────────────────────┬──────────────────────┘
                                                  │  CoreCommand { name, payload, actor, ws }
                                                  ▼
                          ┌──────────────────────────────────────────────┐
                          │  FACADE — forge-core                          │
                          │   WorkspaceCore::handle                        │
                          │     1. authorize(cmd)      (RBAC, auth.rs)     │
                          │     2. Registry::dispatch  (COMMANDS table)    │
                          │     3. cmd_* handler       (one per command)   │
                          └───────────────────────┬──────────────────────┘
                                                  │  pure domain effects + events
                                                  ▼
                          ┌──────────────────────────────────────────────┐
                          │  SUBSYSTEMS — domain · schema · storage ·     │
                          │   crdt · sync · runtime · policy · ui · llm   │
                          └──────────────────────────────────────────────┘
```

Every arrow into the facade is the **same** `CoreCommand`. That is the entire
reason a single CLI can drive "every action": the actions are already unified.

## The layers (forge/ workspace)

| Crate | Role | Relevance to the CLI |
| --- | --- | --- |
| `forge-core` | command/event facade (`handle`, `Registry`, `authorize`) | **the surface the CLI describes & drives** |
| `forge-domain` | `CoreCommand`/`CoreResponse`/`CoreError`/roles | envelope + error types the CLI serializes |
| `forge-schema` | schema registry (collections, fields, migrations) | a source of payload schemas |
| `forge-storage` | Loro-over-SQLite persistence | reached only via commands |
| `forge-crdt` | CRDT merge | via `sync.*` commands |
| `forge-sync` | sync transport | via `sync.export`/`sync.import` |
| `forge-runtime` | TS applet execution + replay | via `runtime.run`/`runtime.replay` |
| `forge-policy` | capability/quota/network policy | informs catalog "capability" + "effectful" |
| `forge-ui` | component-tree UI | via `ui.dispatch_event` |
| `forge-llm` | LLM system | future catalog entries |
| `forge-ffi` | the C ABI | how shells reach the facade |
| `forge-server` | local HTTP (`/bridge`) | the console backend |
| `forge-cli` | the `forge` binary | **what this plan grows** |
| `forge-testkit` | conformance helpers | how we test the catalog |

Design rules that the CLI must honor (from `CLAUDE.md` / `prd-merged`):
deterministic & replayable domain logic; platform effects only at the shell
edge; reuse domain types/errors; no `unwrap`/panics on real paths; keep pure
crates wasm-clean; no private-SaaS concerns in the public engine.

## The front-end surfaces today

### 1. Native shells (5)

All call the **same ABI** and differ only in linking and envelope plumbing
(see [01-FINDINGS.md](01-FINDINGS.md) F4–F5):

| Platform | Language | Links FFI via | Bridge file |
| --- | --- | --- | --- |
| macOS | Swift | dylib (SwiftPM `CForgeCoreBridge`) | `native/macos/.../ForgeCoreBridge.swift` |
| iOS | Swift | staticlib `-force_load` / dylib (sim) | `native/ios/.../ForgeCoreBridge.swift` |
| Windows | C++ | `LoadLibraryW` + `GetProcAddress` | `native/windows/src/ForgeCoreBridge.cpp` |
| Linux | C | `dlopen` + `dlsym` | `native/linux/src/forge_core_bridge.c` |
| Android | C++/JNI | `dlopen("libforge_ffi.so")` + JNI | `native/android/.../forge_core_jni.cpp` |

Each (except Android) also embeds a token-gated loopback `DevControlPlane` that
can already invoke arbitrary commands — proof the "drive commands from outside"
pattern works; the unified CLI standardizes it (F10).

### 2. HTTP server (`forge-server`)

`POST /bridge` is a generic command endpoint; `GET /health`; `POST
/events/drain`; optional bearer auth (F6). This is the **console backend** and a
second way (besides the FFI bin) for the CLI to reach a running core.

### 3. Web runtime (`runtime-web/`) and reference host (`tools/reference-host/`)

`runtime-web/` mounts generated apps and gives them an `AppRuntime.call()`
bridge (app-facing, the *inner* surface). `reference-host` is the Node
conformance harness; its `invokeForgeCore(name, payload)` already drives the
*outer* command surface over the `core-invoke` bin (F7). Both are reusable
building blocks for the console / Node CLI.

### 4. The `forge` CLI (`forge-cli`)

Today: `forge demo` only (F8). This plan turns it into the generic, self-
describing front-end.

## The two command surfaces, mapped onto the platform

```
   OUTER (operator/agent surface) ──── this plan's CLI/console/agent drives this
   ─────────────────────────────
   workspace.*  applet.*  runtime.*  query.*  db.*  schema.*  sync.*
   quota.*  audit.*  package.*  bridge.*  ui.dispatch_event  workspace.export/import
        │
        └── each gated by authorize() roles + capability scopes

   INNER (app/runtime surface) ─────── a running applet uses this; cataloged for docs
   ───────────────────────────
   ctx.db   ctx.net   ctx.files   ctx.ui   ctx.secrets   ctx.timetravel ...
        │
        └── mediated by the bridge + policy (capabilities, quotas, network egress)
```

The catalog (Phase 1) documents **both**, but the CLI's `run` verb targets the
**outer** surface; the **inner** `ctx.*` entries are reference/`describe`-only so
nobody confuses an app capability with an operator action.

## Visibility tiers (preview)

Because "every action" includes privileged ones, the catalog tags each command
with a tier so each front-end can filter correctly:

| Tier | Examples | CLI | Web console (public) | Agent |
| --- | --- | --- | --- | --- |
| `public` | `query.execute`, `runtime.run` | ✅ | ✅ | ✅ |
| `operator` | `applet.install`, `workspace.export` | ✅ | ✅ (authed) | ✅ (authed) |
| `admin` | `quota.set`, `audit.query` | ✅ (authed) | 🔒 opt-in | 🔒 opt-in |
| `debug` | `control.*`, `legacy.core_step` | 🔒 feature-gated | ❌ | ❌ |

Detailed in [10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md).

## Why the platform is *ready* for this

1. **Convergence already happened.** Five shells, one ABI, one registry. There is
   no per-platform command logic to unify — only a description to add.
2. **A generic transport already exists** twice over (FFI `core-invoke` bin and
   HTTP `/bridge`).
3. **RBAC already knows the rules per command** — the catalog reads them, it does
   not invent them.
4. **Determinism is preserved** because introspection is a pure read and the CLI
   only issues commands that already exist.

The unified CLI is therefore not new capability — it is **naming, describing, and
projecting** capability the platform already has.
