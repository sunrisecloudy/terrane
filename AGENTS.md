# Codex working agreements

## Project intent

This repository implements a native WebView platform for AI-generated build-free webapps with Zig core logic. The PRD lives at `docs/00_PRD.md`. Specs are normative; READMEs are not.

## Hard rules

### Generated apps

- Generated webapps must run without a build step.
- Generated webapps may use HTML, CSS, and vanilla JavaScript only unless a specific runtime feature is explicitly added.
- Do not add React, TypeScript, JSX, Vite, Next.js, or npm dependencies to generated app packages.
- Do not invent bridge methods. Use the methods documented in `docs/03_RUNTIME_API_SPEC.md`.
- Every generated app package must include `manifest.json`, `index.html`, `styles.css`, `app.js`, and (recommended) `smoke-tests.json`.
- Every manifest must include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy` (v0.3+).
- `manifest.networkAllowlist` is removed. Use `manifest.networkPolicy` only (v0.4).
- Every interactive element in generated apps must include a stable `data-testid`.
- Generated apps must not call native/platform APIs directly.
- Generated apps must not use direct `fetch`; use `AppRuntime.call("network.request", ...)`.
- Generated apps must not use `localStorage`, `IndexedDB`, or cookies; use storage bridge methods.
- Generated apps must not read or set `appId` in request bodies — the runtime derives it from the per-mount channel (docs/03 §2.1).

### Runtime and hosts

- Runtime dev/control hooks must be compiled out or disabled in production builds.
- Native bridges must apply permission checks; the web runtime check is not sufficient.
- Android bridge must use `WebViewCompat.addWebMessageListener` with an origin allowlist. `addJavascriptInterface` on the runtime WebView is forbidden (docs/05 §4).
- Native hosts must use SQLite via `PlatformDatabase` for persistence. JSON-file storage and SharedPreferences are not allowed (docs/01 §8).
- Production builds must reject `algorithm = "none-dev"` signatures and refuse `--control-plane-port` / `--allow-runtime-mismatch` / `--allow-unsigned-dev` flags.

### Database (v0.4)

- Generated apps never access SQL. `storage.*` is the only persistent surface.
- The runtime derives `app_id` from sandbox context; the calling app cannot choose it.
- App installs are transactional across `apps`, `app_versions`, `app_files`, `app_permissions`, `app_install_reports`, `app_installations`. Either all rows commit or none do.
- Permission changes between versions require approval (docs/17 §9).
- Codex DB inspection goes through `db.snapshot` / `db.query_*` / `db.export_debug_bundle`. Arbitrary SQL is disabled by default.

### Trust and repair

- Generated app source packages are never mounted directly on real targets. Run validate → policy audit → canonicalize → sign → install in one DB transaction → smoke-test → enable.
- Codex repairs must preserve storage compatibility or add migrations.
- Codex must create snapshots before destructive repair / migration / rollback operations.

## Testing expectations

After editing generated apps, run package validation and smoke tests.
After editing runtime bridge behavior, update schemas and bridge contract tests under `tests/fixtures/bridge/`.
After editing `tools/codex-platform-mcp`, run MCP contract tests against the fake host (`tools/fake-platform-host`).
After editing Zig core behavior, run Zig unit tests and replay tests.
After editing native bridge code, re-run the contract suite to confirm the platform still matches the fake-host reference (docs/32 §8).

## Architecture preference

Keep business/domain logic deterministic and replayable. Async/native/platform effects live at the shell edge. Zig core uses event → action state machines. Webapps request actions through a narrow bridge.

## Where to look

- PRD and feature-to-milestone matrix: `docs/00_PRD.md`.
- Bridge surface: `docs/03_RUNTIME_API_SPEC.md`.
- Package format: `docs/04_WEBAPP_PACKAGE_SPEC.md`.
- Platform-specific requirements: `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md`.
- Security boundary: `docs/07_SECURITY_MODEL.md`.
- Database schema: `docs/27_DATABASE_SCHEMA.md`.
- Fake host (reference contract): `docs/32_FAKE_HOST_SPEC.md`.
- Implementation status: `IMPLEMENTATION_STATUS.md` at the repo root.
