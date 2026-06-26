# 09 — Decisions & Open Questions

## Decisions already made (locked in)

| # | Decision | Implication |
|--:|---|---|
| D-1 | **Execute the full A–E program** (not just data extraction). | All 13 steps are in scope; phases sequenced A→E. |
| D-2 | **The Forge core owns app lifecycle** — version history, active-version pointer, and status transitions (rollback/quarantine/activation). | Phase D adds core-owned lifecycle commands + `quota.auto_quarantine`; shell raw SQL on `app_versions.status` becomes illegal; A3 enums + A5 schema are designed toward this end-state. **Command namespace:** see Q8 — legacy webapp registry uses `package.*`, not `applet.*`. |

## Open questions (answer before the noted step)

These do not block Phase A/B; each gates a specific later step. A recommended default is given so the
plan is executable as-is if you simply accept the defaults.

### Q1 — Where does `forge-controlcore` live, and how is it gated? — *gates B6*
A new crate `forge/crates/controlcore` vs a debug-gated module inside `forge-testkit`. And: extend the
existing `COMMANDS` registry with debug-gated `control.*` commands, or add a separate debug FFI entry
point?
- **Recommended default:** a debug-gated module reachable through the **existing** `handle_command`
  seam (new `control.*` commands), **no new FFI entry point**. Keeps the seam contract frozen.

### Q2 — Auto-quarantine policy knobs — *gates D12*
Should the `3 errors / 60s` threshold + prior-version restore become a **core-owned** policy, and
should the threshold/window be **configurable data** vs a fixed constant in core?
- **Recommended default:** core-owned policy; threshold/window as `quota` config (data), defaulting to
  the current `3 / 60s` so behavior is unchanged.

### Q3 — Canonical runtime version — *gates A2 (`runtime-config.json`)*
`WebBridge` reports `0.1.0`; `NativeEngineRoomSnapshotProvider` hard-codes `0.4.0`. Which is canonical,
and should it be a single build-injected constant in `forge-runtime` consumed by all shells?
- **Recommended default:** one build-injected constant from `forge-runtime`; pick the value that
  matches the current shipped runtime contract (likely **`0.4.0`**, the engine-room value, given the
  recent engine-room work — **confirm**). This also fixes bug §D.1.

### Q4 — Public-contract surface for new commands — *gates C8–C10, D12*
Which new commands must appear in `artifacts/public-contract.json` for Premium vs stay internal:
`bridge.validate_envelope`, `bridge.validate_network_request`, `package.get_manifest`,
`package.list_versions/activate/rollback/set_status`, `quota.auto_quarantine`?
- **Recommended default:** app-visible policy/lifecycle commands (network gate, package
  manifest/permissions, webapp package lifecycle, quota) → **in the contract**; debug-only
  `control.*` → **internal**. Existing `applet.*` workspace commands stay as already exported.
  Refresh the Premium pin only after an accepted contract change.

### Q5 — Per-platform capability matrix — *gates A4 (`control-commands.json`)*
Confirm the reduced surfaces are **intentional product constraints** to encode as data, not gaps to
fill: iOS/Android have no app install/uninstall/rollback; Android has no static-HTML analysis (uses
the WebView bridge).
- **Recommended default:** treat them as intentional and encode them in the capability matrix; raise
  any that are actually unintended gaps separately.

### Q6 — Crypto seam scope — *gates E13*
Is unifying token/signature format (custody staying per-platform) **in scope now**, or deferred? It is
security-critical but low-line and orthogonal.
- **Recommended default:** **in scope, last** (do it after A–D). It can be dropped without affecting
  A–D if you prefer to defer.

### Q7 — `forge-controlcore` `wasm32` constraint — *gates B6*
The DevControlPlane logic includes static-HTML parsing; full `wasm32`-cleanliness may need a parser
choice that compiles to wasm.
- **Recommended default:** keep the pure matchers (`jsonMatchesSubset`, snapshot compare, smoke-test
  validation) `wasm32`-clean; gate HTML parsing behind `cfg(not(target_arch = "wasm32"))` if needed.

### Q8 — Legacy webapp packages vs v1 forge applets — *gates A3, C9, D12*
Native shells today maintain a **legacy webapp package registry**
(`apps` / `app_versions` / `app_installations`, file-based HTML/CSS/JS installs) in `platform.sqlite`.
They call Forge only via `legacy.core_step` — not `applet.install`. Forge core separately owns the
**v1 workspace applet model** (`InstalledApplet`, `install_generation`,
`AppletLifecycle::Active/Suspended`) in `forge-workspace.sqlite` per `forge/spec/applet-lifecycle.md`.

Phase D must not conflate these two models. Which command namespace owns the shell registry during
this program?
- **Recommended default:** use a distinct **`package.*` namespace** for the legacy webapp registry;
  leave existing **`applet.*`** commands as the v1 workspace lifecycle (unchanged semantics).

| Concern | Namespace | Examples |
|---|---|---|
| Shell webapp version history, rollback, quarantine, activation | `package.*` | `package.list_versions`, `package.activate_version`, `package.rollback_version`, `package.set_status` |
| Trusted manifest/permissions for an **installed webapp package** | `package.*` | `package.get_manifest`, `package.get_permissions` (C9) |
| v1 workspace applet install/upgrade/suspend/uninstall | `applet.*` (existing) | `applet.install`, `applet.upgrade`, `applet.suspend`, … |

A3 enums for shell statuses should be named **`PackageVersionStatus`** / **`PackageAppStatus`** in
`forge-domain` — not `AppletStatus`, which would collide with existing `AppletLifecycle`. The
generated data file remains `app-status-enums.json` (consumers already expect that filename) but its
Rust source type is `PackageVersionStatus`.

Bridging webapp installs into `applet.install` (full v1 cutover) is **out of scope** for A–E; note
it as a post-D milestone.

### Q9 — Database consolidation strategy — *gates A5, C11, D12*
Shells open **two SQLite files** today: `platform.sqlite` (shell registry, debug tables,
`app_storage`) and `forge-workspace.sqlite` (core `Store`). "One schema" requires choosing how those
files relate during and after this program.
- **Recommended default:** **dual files, unified authority** for A–E:
  - **A5:** one authoritative **migration set** (shared `db/sqlite/` migrations, applied by every
    shell to `platform.sqlite`) replaces hand-maintained `PlatformDatabase` DDL; delete the unsafe
    partial-fallback path. Core workspace tables stay in `forge-workspace.sqlite` as today.
    `tables.json` documents the shell schema; it does not merge the two files.
  - **C11 / D12:** core commands own **writes** to shell-registry tables (`apps`, `app_versions`,
    `app_installations`, debug audit tables). Shells supply DB transport (open `platform.sqlite`,
    execute core-issued SQL or call through the existing FFI with the platform DB path) but stop
    issuing ad-hoc `UPDATE app_versions.status` themselves.
  - **`app_storage`:** rows stay in `platform.sqlite` through A–E. Prefix/quota **decisions** move
    to core (C10); physical read/write stays shell-transported. Moving `app_storage` into forge KV
    is a **follow-on**, not this program.
  - **Single-file merge** (one `.sqlite` for workspace + platform tables) is **deferred post-D**
    until lifecycle authority is proven and backup/export semantics are updated.

### Q10 — `reference-host` scope — *gates A4, B6–B7, D12*
`tools/reference-host/src/platform-database.js` reimplements the same app-registry,
rollback/quarantine, and control-plane logic as the five native shells. It is the Node conformance
oracle (`tools/reference-host/`, MCP dev-control, CI smoke). The plan originally covered five shells
only.
- **Recommended default:** **in scope as a sixth first-class consumer**, migrated **in parallel with
  macOS** for each relevant phase:
  - **A4:** `control-commands.json` capability matrix + `control-response-schema.json` apply to
    reference-host routes (not only native shells).
  - **B6–B7:** reference-host DevControlPlane pure logic delegates to `forge-controlcore`; same
    golden vectors as macOS; delete duplicated JS algorithm code.
  - **D12:** reference-host stops raw SQL on `app_versions`; calls `package.*` core commands.
  - **Fan-out order:** macOS **and** reference-host per phase (paired commits or one commit covering
    both), then iOS → Linux → Windows → Android one shell per commit.

---

## How to record answers

When you decide, update [10-validation-and-sequencing.md](10-validation-and-sequencing.md) if it
changes a gate, and note the answer here under the question. The defaults above are safe to proceed
on if you take no action — the plan is executable as written.
