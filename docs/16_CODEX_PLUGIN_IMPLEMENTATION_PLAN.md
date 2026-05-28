# Codex Plugin Implementation Plan

## Implementation principle

The control plugin should be implemented before large-scale AI app generation. Without it, AI-generated apps cannot be repaired reliably.

## Milestone 1: Repository guidance

Add `AGENTS.md` at the repository root with these rules:

- Generated apps must be build-free.
- Do not add npm-only runtime dependencies to generated apps.
- Use `data-testid` for testable elements.
- Do not invent bridge methods.
- Always update schemas and tests when changing protocol.
- Run package validation after editing `webapps/`.
- Run MCP contract tests after editing `tools/codex-platform-mcp/`.

## Milestone 2: Fake host first

Before native hosts expose the real control plane, implement a fake host:

```text
tools/fake-platform-host/
  src/server.ts
  src/runtime-simulator.ts
  src/fixtures.ts
```

The fake host lets Codex and CI test the MCP server without macOS/iOS/Android/Windows/Linux toolchains.

Minimum fake host support:

- Install package from path.
- Load `index.html` in a headless DOM environment or browser fixture.
- Simulate `AppRuntime.call`.
- Capture console, bridge calls, storage, and events.
- Run micro-tests.

## Milestone 3: MCP server

Implement `tools/codex-platform-mcp`.

Recommended stack:

- TypeScript or JavaScript.
- Model Context Protocol SDK.
- JSON Schema validation for tool inputs.
- HTTP client to the platform control plane.
- Contract tests against the fake host.

The MCP server should not contain business logic. It translates MCP tool calls into control-plane commands and normalizes responses.

## Milestone 4: Runtime dev hooks

Add development-only hooks to `runtime-web`:

- `window.__APP_RUNTIME_DEVTOOLS__.snapshot()`
- `window.__APP_RUNTIME_DEVTOOLS__.query(...)`
- `window.__APP_RUNTIME_DEVTOOLS__.bridgeLog()`
- `window.__APP_RUNTIME_DEVTOOLS__.consoleLog()`
- `window.__APP_RUNTIME_DEVTOOLS__.storageSnapshot(appId)`
- `window.__APP_RUNTIME_DEVTOOLS__.coreEventLog()`
- `window.__APP_RUNTIME_DEVTOOLS__.reset(appId)`

These hooks must only exist in dev/test mode.

## Milestone 5: Desktop adapters

Implement macOS, Linux, and Windows adapters first because they are easiest to attach to.

Desktop host dev mode:

```text
--dev-control
--control-port 29371
--control-token <random>
--runtime-dir ./runtime-web
--webapps-dir ./webapps/examples
```

## Milestone 6: Mobile simulator/emulator adapters

Add Android emulator and iOS simulator support.

Android adapter responsibilities:

- Build debug APK.
- Install APK.
- Launch activity with dev-control extras.
- Establish port forwarding/tunnel.
- Attach MCP control session.

IOS simulator adapter responsibilities:

- Build simulator app.
- Boot simulator if needed.
- Install app.
- Launch app with dev-control arguments/environment.
- Establish control session.

Physical devices are optional in v0.1.

## Milestone 7: Codex plugin package

Create local plugin:

```text
codex-plugin/platform-control/
  .codex-plugin/plugin.json
  .mcp.json
  skills/
```

Add local marketplace entry:

```text
.agents/plugins/marketplace.json
```

Codex should be able to install/enable the plugin and then run prompts like:

```text
Use @platform-micro-test to install webapps/examples/notes-lite and run its micro-tests on the macOS host.
```

## Milestone 8: Repair loop

Implement a Codex workflow that:

1. Runs a micro-test.
2. Reads failure bundle.
3. Determines whether the bug is generated app, runtime, bridge, native host, or Zig core.
4. Patches the correct layer.
5. Runs targeted test.
6. Runs full relevant suite.
7. Produces a short verification report.

## Milestone 9: Hardening

- Compile out control endpoints in production.
- Require token on every request.
- Add rate limits.
- Add audit log.
- Add destructive-command confirmation.
- Add command allowlist.
- Add tests proving generated apps cannot access control APIs.

## v0.3 plugin implementation additions

Add MCP tool handlers for:

- `platform.sign_webapp_package`
- `platform.install_report`
- `platform.list_webapp_versions`
- `platform.rollback_webapp`
- `platform.quarantine_webapp`
- `platform.create_snapshot`
- `platform.restore_snapshot`
- `platform.run_policy_audit`
- `platform.run_repair_loop`
- `runtime.capabilities`
- `runtime.compare_snapshot`
- `runtime.resource_usage`
- `runtime.run_accessibility_audit`
- `runtime.accessibility_snapshot`

Implementation order:

1. Fake-host only.
2. Browser/runtime mock.
3. macOS/Linux desktop dev hosts.
4. Android/iOS simulator targets.
5. Windows host.

The fake-host implementation is the reference behavior for contract tests.

## Database tool implementation

Add database tools after fake-host persistence exists:

1. Implement safe control-plane handlers for DB queries.
2. Add MCP tool names to `tools/codex-platform-mcp/src/tool-contract.ts`.
3. Add tests proving the tools return normalized `ControlResponse` objects.
4. Add micro-test assertions that use DB tools.
5. Add `db.export_debug_bundle` to repair-loop diagnostics.

Do not add arbitrary SQL support in the default plugin.
