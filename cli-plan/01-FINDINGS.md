# 01 — Findings (the evidence)

This is the audit that justifies the plan. Every claim is backed by a
`file:line` citation against the `main` snapshot. Re-verify before coding.

## F1 — There is exactly one command facade

Every action is a `CoreCommand { request_id, actor, workspace_id, applet_id,
name, payload }` handled by `WorkspaceCore::handle`.

- Envelope type: `forge/crates/domain/src/lib.rs:138` (name + `serde_json::Value`
  payload + actor context + workspace id).
- Dispatch: `forge/crates/core/src/commands/mod.rs` — `handle` runs the RBAC gate
  then calls `Registry::dispatch` (`commands/mod.rs:212`).

**Implication:** there is a single chokepoint to describe and to expose. Nothing
about "every action" is scattered.

## F2 — Dispatch is already a data-driven table

The registry is a `&[(&str, Handler)]` table, not a hand-written match.

- `COMMANDS` table: `forge/crates/core/src/commands/mod.rs:68` — ~46 entries,
  each `("name", WorkspaceCore::cmd_x)`.
- Debug-gated `CONTROL_COMMANDS` (feature `control`): `commands/mod.rs:150` — 9
  more entries.
- Unknown name → identical `ValidationError` reject (CR-A5): `commands/mod.rs:224`.

**Implication:** adding a metadata field per entry is a localized, mechanical
change — the table is *already* the catalog, minus the metadata.

## F3 — Required role is already a per-command table (just separate)

Authorization is a static match on `cmd.name` → allowed roles, run *before*
dispatch.

- `authorize(cmd)`: `forge/crates/core/src/auth.rs:28`.
- Examples: `workspace.create` → `[Owner]` (`auth.rs:33`); `query.execute` →
  `[Owner, Maintainer, Editor, Viewer, Auditor]` (`auth.rs:99`); `runtime.run`
  → `[Owner, Maintainer, Editor, Runner]` (`auth.rs:57`); unknown → `None`
  (`auth.rs:183`).
- Secondary capability gates (collection-scoped `db.read`/`db.write`):
  `auth.rs:239` and `auth.rs:341`.

**Implication:** the "required role" metadata the catalog needs *already exists
in code*. The catalog should **reference / derive** it, not duplicate it — ideally
`authorize` and the catalog read the same table (see
[10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md)).

## F4 — One C ABI; every native shell already routes through it

- ABI exports: `forge/crates/ffi/src/lib.rs` — `forge_core_open` (`:283`),
  `forge_core_open_in_memory` (`:312`), `forge_core_handle_command` (`:342`),
  `forge_core_drain_events` (`:366`), `forge_core_last_error` (`:378`),
  `forge_core_close` (`:394`).
- A generic stdin invoker already exists: `forge/crates/ffi/src/bin/core_invoke.rs`
  reads a full `CoreCommand` JSON on stdin and runs it.

Per-shell call sites (all pass a JSON envelope to `*_handle_command`, no
hand-built routing):

- macOS: `native/macos/Sources/TerraneHostMac/ForgeCoreBridge.swift:74`
  (`commandEnvelope()` at `:292`).
- iOS: `native/ios/Sources/TerraneHostIOS/ForgeCoreBridge.swift:51`
  (`commandEnvelope()` at `:181`).
- Windows: `native/windows/src/ForgeCoreBridge.cpp:180` (envelope at `:349`).
- Linux: `native/linux/src/forge_core_bridge.c:73` (envelope at `:230`).
- Android: `native/android/app/src/main/cpp/forge_core_jni.cpp:134`.

**Implication:** goal (1) — a unified call system across platforms — is already
satisfied. The shells differ only in *linking* (dylib vs `LoadLibraryW` vs
`dlopen`/JNI), not in *what* they call.

## F5 — Command envelope construction is duplicated per shell

Each shell re-implements the JSON envelope (`request_id`, `actor`,
`workspace_id`, `name`, `payload`) independently — macOS `:292`, iOS `:181`,
Windows `:349`, Linux `:230`, Android inline `:134`. There is **no shared
helper**.

**Implication:** a self-describing catalog plus a thin shared command-builder is
also an opportunity to de-duplicate envelope logic later (out of scope for v1,
noted in [13-OPEN-QUESTIONS.md](13-OPEN-QUESTIONS.md)).

## F6 — The HTTP server already exposes a generic command endpoint

- `POST /bridge` deserializes a `CoreCommand` and calls `core.handle`:
  `forge/crates/server/src/lib.rs:92`–`135` (server injects its own actor /
  workspace, ignoring client-supplied identity).
- `GET /health`: `server/src/lib.rs:84`. `POST /events/drain`:
  `server/src/lib.rs:96`.
- Optional bearer-token auth (`Authorization: Bearer` or `x-forge-server-token`),
  401 with `www-authenticate`: `server/src/lib.rs:157`.

**Implication:** goal (3) — a web console — has its backend already. The console
is a static page that POSTs to `/bridge`. No new server protocol needed.

## F7 — Existing JS harness already invokes commands

- `invokeForgeCore(name, payload, options)` wraps any command and shells out to
  the `core-invoke` bin: `tools/reference-host/src/forge-core-bridge.js:42`–`65`.
- The reference host already calls ~15 commands this way (e.g.
  `bridge.validate_envelope`, `package.get_permissions`).

**Implication:** there is a working JS pattern to reuse for the console / Node
CLI, and a conformance harness that already exercises the surface.

## F8 — The library half of the CLI already exists; only argv is thin

- `forge` binary parses **only** `demo` today:
  `forge/crates/cli/src/main.rs:14`–`26`.
- But the lib has the generic pieces: `handle(core, applet_id, name, payload)`
  (`forge/crates/cli/src/lib.rs:186`), `install(...)` (`:158`),
  `list_records(...)` (`:212`).

**Implication:** goal (2) — a CLI that runs every command — is a small front-end
addition over functions that already exist.

## F9 — The gap: nothing is machine-readable per command

This is the **one real gap**, and everything in the plan builds on closing it.

- The registry entry is `(name, handler)` only — no schema, role, or effect
  metadata: `commands/mod.rs:68`.
- Per-command payload/response contracts live in **human-readable markdown**, not
  schemas: `forge/spec/commands.md` (table of name → request fields → response →
  roles). It explicitly notes "There are no per-command Rust request/response
  structs yet" (`forge/spec/commands.md:52`).
- The public contract enumerates **command names only**, from hardcoded arrays —
  no schemas: `tools/export-public-contract.mjs:86`–`121` (`bridge.methods`),
  `:213`–`243` (`generatedAppBoundary.api` / `ctx.*`). Verify checks hashes/
  presence, not schema correctness: `tools/verify-public-contract.mjs`.
- Domain `*.schema.json` files exist for *objects* (manifests, records, bridge
  contracts — ~23 files under `schemas/`) but there is **no name → schema
  registry** for commands.

**Implication:** the keystone work (Phase 1, [05](05-PHASE-1-SELF-DESCRIBING-REGISTRY.md))
is to attach machine-readable metadata to each registry entry and make it the
single source the CLI, console, agent, *and* the public contract all read.

## F10 — Dev control planes already prove "arbitrary command invocation"

macOS, iOS, Windows, and Linux each ship a token-gated, loopback `DevControlPlane`
HTTP server that can already invoke arbitrary commands (e.g. `runtime.core_step`,
bridge routing) — e.g. iOS `IOSDevControlPlane.swift` `dispatchCommand()`. Android
does **not** have one.

**Implication:** the *capability* to drive arbitrary commands from outside the
shell is proven and shipped; the unified CLI generalizes and standardizes it
rather than inventing it. (Note: `/control` is being retired per the
control-surface decision — the unified CLI is its principled replacement, not an
extension of it.)

## Summary table

| Need | Status | Evidence |
| --- | --- | --- |
| One facade for every action | ✅ exists | F1 |
| Data-driven dispatch table | ✅ exists | F2 |
| Per-command required role | ✅ exists (separate) | F3 |
| One ABI all shells use | ✅ exists | F4 |
| Generic HTTP command endpoint | ✅ exists | F6 |
| JS invoke harness | ✅ exists | F7 |
| Generic CLI library functions | ✅ exists | F8 |
| Generic CLI front-end (argv) | ⚠️ only `demo` | F8 |
| Machine-readable per-command metadata | ❌ **the gap** | F9 |
| Auto-generated console / agent surface | ❌ depends on the gap | F6, F9 |
