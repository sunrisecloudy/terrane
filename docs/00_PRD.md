# PRD: Native AI Webapp Platform v0.4

## 0. Document conventions

- Each requirement is tagged with the milestone in which it first becomes load-bearing: **[v0.1]**, **[v0.2]**, **[v0.3]**, **[v0.4]**.
- "Generated app" means a webapp package produced by an AI agent or by a developer outside the runtime build pipeline.
- "Host" means a native or server process that owns a WebView (or HTTP surface for the server) and the bridge dispatcher.
- "Runtime" means the build-free HTML/CSS/vanilla JS layer loaded inside the host WebView.
- Section 0.2 contains a Feature → Milestone matrix; the rest of the document elaborates each row.

## 0.1 Spec versioning

| Version | Theme | Status |
|---|---|---|
| v0.1 | Cross-platform shells, Zig core, fixed bridge, 5 example apps | normative |
| v0.2 | Codex control plugin + dev control plane | normative |
| v0.3 | Signing, immutable installs, rollback, capabilities, resource budgets, network policy, accessibility, snapshots | normative |
| v0.4 | Formal SQLite/Postgres persistence layer, install/version transactions, backup/export/import | normative |

The current PRD is v0.4. All sections below that mention earlier milestones are still required at v0.4; they are tagged to show when the requirement first appeared.

## 0.2 Feature → Milestone matrix

| Feature | First required | Doc anchor |
|---|---|---|
| Cross-platform host shells (iOS, macOS, Android, Windows, Linux) | v0.1 | §5 G1 |
| Zig core FFI (`core_create`/`core_step_json`/`core_free`) | v0.1 | §5 G2 |
| Build-free webapp package format | v0.1 | §5 G3 |
| Fixed bridge methods | v0.1 | §5 G4 |
| Sandboxed iframe execution | v0.1 | §5 G5 |
| 5 reference example apps | v0.1 | §5 G6 |
| Server (Zig HTTP) parity for `core.step` | v0.1 | §5 G1 |
| Reference host as reference contract | v0.1 | docs/32_REFERENCE_HOST_SPEC.md |
| Codex MCP plugin and dev control plane | v0.2 | §5 G7 |
| Micro-test protocol | v0.2 | docs/15_MICRO_TESTING_PROTOCOL.md |
| `data-testid` selectors in examples | v0.2 | docs/15 |
| Package canonicalization + signing | v0.3 | docs/17_APP_SIGNING_AND_TRUST.md |
| Immutable installed app versions | v0.3 | docs/18 |
| Rollback and quarantine | v0.3 | docs/18 |
| Per-app `dataVersion` + storage migrations | v0.3 | docs/19 |
| Runtime capability negotiation | v0.3 | docs/20, docs/03 §9 |
| Resource budgets (DOM, storage, bridge, network, timers, logs) | v0.3 | docs/22 |
| Network policy (replaces `networkAllowlist`) | v0.3 | docs/24 |
| Accessibility contract | v0.3 | docs/23 |
| Snapshot/replay format | v0.3 | docs/21 |
| Platform capability matrix per target | v0.3 | docs/26 |
| Persistence layer (SQLite + Postgres logical schema) | v0.4 | docs/27 |
| Install transaction across `apps/app_versions/app_files/app_permissions/app_installations/app_install_reports` | v0.4 | docs/27 §6 |
| `app_storage(app_id, key, value_json)` as canonical generated-app store | v0.4 | docs/28 |
| Backup/export/import | v0.4 | docs/29 |
| Codex DB inspection tools (`db.snapshot`, etc.) | v0.4 | docs/14 |

## 1. Product summary

Build a cross-platform native application platform that can generate, install, and run small-to-complex webapps on the fly using AI-generated HTML/CSS/vanilla JS, while keeping platform power, storage, networking, and core business logic behind a fixed native/Zig bridge.

The platform must support **[v0.1]**:

- iOS app shell.
- macOS app shell.
- Android app shell.
- Windows desktop shell.
- Linux desktop shell.
- Zig server executable.
- Shared Zig core logic.
- Multiple generated webapps loaded dynamically without a build step.

A **reference host** (Node + jsdom-equivalent or browser-only) is the reference implementation of the bridge contract. Native hosts must match it byte-for-byte on bridge responses.

The first version is not a full app builder. It is a minimal but real runtime that proves the full architecture works on every platform.

## 2. Core thesis

AI should generate app packages, not native platform code.

```text
AI output:
  manifest.json
  index.html
  styles.css
  app.js
  smoke-tests.json
  migrations/*.json (optional, v0.3+)

Runtime output:
  validated app
  canonicalized + signed installed version (v0.3+)
  sandboxed execution
  bridge calls
  actions sent to native/Zig
  audit log + install report persisted in DB (v0.4+)
```

This avoids per-generated-app build steps and works across WebViews.

## 3. Users

### Tier-0 actor: AI coding agents (Codex and equivalents)

The platform is designed first for AI agents that generate, validate, install, micro-test, and repair webapps. The Codex control plugin and dev control plane (§5 G7) are first-class surfaces, not afterthoughts. Most install/repair/test flows are expected to run agent-first.

### Primary human user

A technical product builder who wants to generate many mini-apps or internal tools inside one native shell and reuse the same core logic across desktop, mobile, and server.

### Secondary users

- Developers implementing platform-specific native shells.
- Developers maintaining Zig core logic.
- QA running compatibility tests across platforms.
- End users who install generated apps inside the host (consumer of mini-apps).

## 4. Non-goals for v0.1

- No arbitrary npm dependency installation for generated apps.
- No TypeScript, JSX, React, Vite, Next.js, or bundler inside the runtime.
- No direct native API access from generated apps.
- No marketplace, payments, multi-user sharing, or signing service.
- No visual no-code editor.
- No attempt to make Zig own platform UI or mobile lifecycle.
- No full offline sync engine unless a demo app needs a tiny mocked version.
- No download-and-run of arbitrary remote third-party packages on iOS App Store builds (see §8 decision D1).

## 5. MVP goals

### G1. Cross-platform host shells **[v0.1]**

Each platform must ship a minimal native host that can:

- Create a WebView (or HTTP surface for the server).
- Load `runtime/index.html` from the app bundle.
- Inject or expose a native bridge that satisfies the contract in `docs/03_RUNTIME_API_SPEC.md`.
- Route runtime bridge calls to platform services and Zig core.
- Load bundled example webapps.
- Persist app data (see G8 once v0.4 lands).
- Display logs/errors during development.

Supported targets:

| Platform | Host technology | WebView | Bridge mechanism |
|---|---|---|---|
| iOS | Swift | WKWebView | `WKScriptMessageHandlerWithReply` |
| macOS | Swift | WKWebView | `WKScriptMessageHandlerWithReply` |
| Android | Kotlin | Android WebView | `WebViewCompat.addWebMessageListener` (NOT `addJavascriptInterface`; see docs/05) |
| Windows | C++/WinRT | WebView2 | `WebMessageReceived` |
| Linux | C + GTK4 | WebKitGTK | `WebKitUserContentManager` script-message handlers |
| Server | Zig | HTTP/JSON | POST `/bridge` (control-plane endpoints separate) |

### G2. Shared Zig core **[v0.1]**

The Zig core must expose a tiny coarse API:

- `core_create`
- `core_destroy`
- `core_step_json`
- `core_free`

The core accepts an event and returns actions. It must be deterministic for the same state and event stream.

### G3. Build-free webapp packages **[v0.1]**

Generated apps must run without a build step. A valid v0.1 package contains:

- `manifest.json`
- `index.html`
- `styles.css`
- `app.js`
- `smoke-tests.json` (optional but recommended)
- `assets/` (binary asset support arrives in v0.5; not allowed in v0.1)
- `migrations/` (introduced v0.3)

### G4. Fixed runtime bridge **[v0.1]**

Generated apps may only call:

```js
await AppRuntime.call(method, params)
```

Allowed v0.1 methods:

- `core.step`
- `storage.get`
- `storage.set`
- `storage.remove`
- `storage.list`
- `dialog.openFile`
- `dialog.saveFile`
- `notification.toast`
- `network.request`
- `app.log`

v0.3 adds `runtime.capabilities` (host-mediated).

The runtime and native host must reject unknown methods with `unknown_method`.

### G5. Security boundaries **[v0.1, hardened through v0.3]**

The system must enforce:

- Per-app manifest permissions.
- App storage namespaces, with app id derived by the runtime (not by the calling page).
- Sandboxed webapp execution (iframe with `sandbox="allow-scripts"`, no `allow-same-origin`).
- CSP/policy checks (see docs/07 §8).
- HTML/script policy checks at install time.
- No direct `fetch`, `localStorage`, `IndexedDB`, cookies, or native APIs from generated apps.
- Resource quotas (v0.3 budgets) for generated apps.

### G6. Reference example apps **[v0.1]**

The first version must include 5 example webapps:

1. **Notes Lite** — `storage.*`, search, toast.
2. **Task Workbench** — `core.step`, storage, validation, stateful workflows.
3. **File Transformer** — `dialog.openFile`, `core.step` transform, `dialog.saveFile`.
4. **API Dashboard** — `network.request`, table/chart, storage, notifications.
5. **Core Replay Lab** — `core.step`, event log, replay, export.

Coverage rationale and bridge-method matrix live in `docs/13_EXAMPLE_APP_COVERAGE.md`.

### G7. Codex control plugin and dev control plane **[v0.2]**

v0.2 must include a developer-only control plane and a Codex plugin skeleton so Codex can granularly test generated apps while implementing the platform.

The system must provide:

- A Codex plugin package with a skill and MCP server configuration.
- A local MCP server that Codex can call from CLI or IDE.
- A dev-only host control plane exposed by native desktop shells and by mobile simulator/emulator builds.
- A stable micro-test protocol for UI, bridge, storage, network, timer, and Zig core behavior.
- Tooling to install a generated webapp package, open it, inspect it, drive it, assert behavior, and replay failures.

Workflows supported by the plugin:

1. Generate a webapp package.
2. Validate its manifest, HTML/CSS/JS policy, and smoke tests.
3. Install it into a running platform host.
4. Open the app inside the sandboxed WebView runtime.
5. Drive UI at selector-level granularity.
6. Inspect runtime state, DOM state, console logs, bridge calls, storage, and core event/action logs.
7. Mock external effects such as network responses, file dialogs, notification delivery, and timers.
8. Run micro-tests and repair the app package until tests pass.
9. Run platform smoke tests on iOS simulator, Android emulator, macOS, Windows, Linux, and server adapter where available.

Non-goals for the plugin:

- No production remote-control endpoint.
- No uncontrolled native command execution through generated apps.
- No arbitrary browser JavaScript evaluation unless explicitly enabled in a dev-only unsafe mode named `runtime.unsafe_eval` and disabled in CI.
- No direct bypass of the manifest permission system.

The Codex-facing plugin is part of the implementation workflow, not part of the shipped user-facing feature set.

### G8. Database, persistence, and migration layer **[v0.4]**

The platform must include a formal persistence layer. Native hosts and the reference host use SQLite. The server supports SQLite for development and a Postgres-compatible schema for production. Generated apps never create SQL tables or access the database directly. All generated app data goes through the storage bridge and is persisted as namespaced key/value JSON in `app_storage`.

The platform database also stores:

- app registry metadata;
- immutable app versions;
- app package files;
- app permissions;
- app installations and activation/rollback history;
- app install reports;
- app migrations and migration runs;
- bridge/core logs;
- runtime sessions and snapshots;
- test runs;
- Codex control sessions and commands;
- network/dialog mocks;
- backup/export/import records.

Schema, transaction rules, and indexes live in `docs/27_DATABASE_SCHEMA.md`.

## 6. Success criteria

A coherent release is acceptable when:

- All platform shells launch.
- All shells show the runtime launcher.
- All 5 example apps load.
- All allowed bridge methods work or intentionally return a clear `platform_unsupported` error on platforms where they cannot be supported yet.
- Unknown bridge methods are denied with `unknown_method`.
- Permission-denied paths are visible and tested.
- Zig core can be called from every native shell and from the server.
- The runtime can validate and reject invalid webapp packages.
- Smoke tests run for every example app.
- The server can run the same core step contract.
- **[v0.3]** Apps install only through the canonicalize → sign → install → smoke-test pipeline. Tampered packages fail mount.
- **[v0.3]** A failing micro-test quarantines the new version and keeps the previous version active.
- **[v0.4]** All persistent state (`apps`, `app_versions`, `app_files`, `app_permissions`, `app_installations`, `app_storage`, `app_install_reports`, `bridge_calls`, `core_events`, `core_actions`, `runtime_snapshots`, `test_runs`, `control_sessions`, `control_commands`, `network_mocks`, `dialog_mocks`, `app_migrations`, `migration_runs`, `backup_exports`) lives in the platform DB across native + server + reference host.
- **[v0.4]** Backup export/import round-trips one example app on the reference host without data loss.

## 7. Product principles

1. **Runtime API stability over generated-code freedom.** Generated apps must adapt to the runtime, not vice versa.
2. **Coarse boundaries.** Keep native/Zig/webapp boundaries message-based.
3. **No build step for generated apps.** Generation must produce files that can run directly.
4. **AI output must be validated before install.** Never trust generated HTML/JS.
5. **Core logic stays deterministic.** Platform effects happen outside the Zig core.
6. **Every platform proves the same contract.** Platform shells may differ internally, but the web runtime and core API stay identical.
7. **Reference contract first, native hosts second.** Build the reference host so each native shell can be diffed against a known-good baseline.
8. **Persistence is platform-owned, not app-owned.** Generated apps see only `storage.*`; SQL never leaks across the boundary.

## 8. Decisions and open questions

### Decisions (closed)

- **D1. iOS App Store distribution model.** Per Apple App Store Review Guideline 4.7 (clarified Nov 2025), HTML5/JavaScript mini-apps and mini-games are in scope for App Store review. v0.1 App Store builds of the iOS host ship only the 5 first-party reviewed bundled apps. AI-generated and user-installed packages are gated to TestFlight and Developer-ID/sideloaded builds only. v0.1 native hosts must not extend or expose native platform APIs to non-bundled apps beyond the methods listed in G4 (per Guideline 4.7.2). Any change to this model requires explicit Apple permission and a PRD update.
- **D2. Mini-app indexing and age gating.** When iOS bundled-app distribution is enabled, the host must implement an index of installed mini-apps (id, title, description, version, content rating) per Guideline 4.7.4 and an age-restriction gate per Guideline 4.7.5.
- **D3. Android bridge mechanism.** Production Android builds must use `WebViewCompat.addWebMessageListener` with an origin allowlist. `addJavascriptInterface` is forbidden because it cannot verify the calling frame origin and has historical RCE vectors (see docs/05).
- **D4. Storage backend.** SQLite is canonical for all hosts (iOS, macOS, Android, Windows, Linux, reference host, server dev). The server uses a Postgres-compatible logical schema in production. JSON-file storage is not supported.
- **D5. Capabilities API form.** Generated apps and Codex call `AppRuntime.call("runtime.capabilities", {})`. `AppRuntime.capabilities()` is a thin convenience wrapper that delegates to the same bridge method. There is one source of truth.
- **D6. Network policy field.** `manifest.networkPolicy` is the only normative network-allow surface. `manifest.networkAllowlist` is removed; it is no longer accepted as input (see docs/04 and docs/24).

### Open (deferred past v0.4)

- Whether to support user-downloaded apps on iOS App Store builds (D1 may be relaxed if Apple grants explicit permission).
- Whether to support Preact/HTM as an advanced no-build mode.
- Whether core messages should move from JSON to CBOR/MessagePack.
- Whether server and native app share a package registry.
- Whether webapps can run background jobs while inactive.
- Whether generated apps may include `assets/` binary files (planned v0.5).
- Whether to support an alternative WebAssembly-loaded Zig core for the runtime (vs. native FFI).

## 9. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| iOS App Store rejection under Guideline 4.7 | Blocks consumer distribution | D1 limits initial release to bundled-only mini-apps |
| Bridge contract drift between hosts | Generated apps work on one platform but not another | Reference host as reference + per-method contract fixtures (docs/08 §6) |
| Generated app abuses storage/network/timers | DoS, exfiltration | Manifest permissions + budgets + audit log + quarantine |
| SQLite corruption on mobile | Data loss | `PlatformDatabase.open()` runs integrity check + supports re-init from backup_export |
| Zig core determinism drift across compilers | Replay test failures | Pin Zig toolchain in CI; fixtures committed |
| Codex over-permissive control plane | Dev token leak ⇒ remote control | Local bind + per-launch token + audit log + production builds compile control plane out |

## 10. Versioning of this PRD

| PRD revision | Date | Change |
|---|---|---|
| v0.1 | 2026-04 | Initial: shells, core, bridge, examples |
| v0.2 | 2026-04 | Added Codex control plugin and dev control plane (G7) |
| v0.3 | 2026-05 | Added signing, immutable installs, rollback, capabilities, budgets, network policy, accessibility, snapshots |
| v0.4 | 2026-05 | Added persistence layer; renumbered goals; resolved D1–D6 |
