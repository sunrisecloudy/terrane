# Webapp Package Spec

This document is normative for **v0.4**. Sections tagged **[v0.1]**/**[v0.3]** are the milestone in which the requirement first appeared. All sections apply at v0.4 unless explicitly noted.

## 1. Package contents

A v0.4 generated webapp source package contains:

```text
manifest.json             required
index.html                required
styles.css                required
app.js                    required
smoke-tests.json          optional but recommended
migrations/*.json         optional (v0.3+; required when dataVersion increases)
assets/                   not allowed in v0.1–v0.4 (planned v0.5)
```

The installed package adds platform-generated files (`signature.json`, `install-report.json`, `content-hashes.json`) per docs/17.

No build step is allowed.

## 2. Manifest shape

```json
{
  "id": "notes-lite",
  "name": "Notes Lite",
  "version": "0.1.0",
  "runtimeVersion": "0.1.0",
  "dataVersion": 1,
  "entry": "index.html",
  "description": "Simple notes app",
  "permissions": [
    "storage.read",
    "storage.write",
    "notification.toast",
    "app.log"
  ],
  "storagePrefix": "notes-lite:",
  "capabilities": {
    "required": ["storage.read", "storage.write"],
    "optional": ["notification.toast"]
  },
  "resourceBudget": {
    "maxDomNodes": 2000,
    "maxStorageBytes": 5242880,
    "maxBridgeCallsPerMinute": 600,
    "maxNetworkRequestsPerMinute": 60,
    "maxTimers": 64,
    "maxLogLinesPerMinute": 120,
    "maxPackageBytes": 1048576,
    "maxFileBytes": 524288
  },
  "networkPolicy": {
    "allow": []
  }
}
```

Field reference:

| Field | Required since | Notes |
|---|---|---|
| `id` | v0.1 | Lowercase kebab-case, 3–64 chars, `^[a-z][a-z0-9-]{2,63}$` |
| `name` | v0.1 | Human-readable title |
| `version` | v0.1 | Semver of the generated app itself |
| `runtimeVersion` | v0.1 | Semver of the runtime API this app expects; see §8 |
| `dataVersion` | v0.3 | Positive integer; bump when storage shape changes |
| `entry` | v0.1 | Must equal `index.html` in v0.4 |
| `description` | v0.1 | One-line summary |
| `permissions` | v0.1 | Subset of permission table in docs/03 §4 |
| `storagePrefix` | v0.1 | Must equal `<id>:` |
| `capabilities.required` | v0.3 | Capability ids the app cannot run without |
| `capabilities.optional` | v0.3 | Capability ids the app can degrade without |
| `resourceBudget` | v0.3 | All keys required (see docs/22) |
| `networkPolicy` | v0.3 | Object with `allow` array; see docs/24 |

`networkAllowlist` is **removed** as of v0.4 (decision D6 in docs/00 §8). Validators must reject packages that include it.

## 3. Generated HTML rules **[v0.1]**

Allowed:

- Semantic HTML.
- Standard form elements.
- Relative links to package files.
- Script tag loading `app.js` (must be `<script src="app.js"></script>`, no `type="module"`, no `defer`-only logic).
- Link tag loading `styles.css`.

Disallowed:

- Remote scripts.
- Remote stylesheets.
- Inline event handlers such as `onclick="..."`.
- Inline `<script>` blocks of any kind.
- `iframe` inside generated apps.
- `object`, `embed`, `applet`.
- Forms that submit to the network directly (`action` must be `#` or absent).
- `javascript:` URLs.
- Meta refresh.
- Service workers.

## 4. Generated JS rules **[v0.1]**

Allowed:

- Vanilla JavaScript.
- DOM APIs.
- `AppRuntime.call`.
- `AppRuntime.on` if implemented (event list in docs/03 §1.1).
- Timers within quota.

Disallowed:

- `eval`.
- `new Function`.
- Dynamic `import()`.
- Direct `fetch`.
- `XMLHttpRequest`.
- WebSocket / EventSource.
- `localStorage` / `sessionStorage`.
- IndexedDB.
- Cookies.
- Direct native bridge access (e.g., `webkit.messageHandlers`, `chrome.webview`, `Android.*`).
- Accessing `window.parent`, `window.top`, or `window.opener` except through the runtime bridge.
- Trusted Types policy creation outside the runtime's policy (see docs/07 §8).

## 5. Generated CSS rules **[v0.1]**

Allowed:

- Self-contained CSS.
- CSS variables.
- Responsive media queries.
- Light/dark themes via `prefers-color-scheme`.

Disallowed:

- Remote `@import`.
- External fonts in v0.1–v0.4.
- Positioning designed to escape the app viewport (`position: fixed` outside the host frame is rejected).
- `url()` references to non-relative paths or non-package files.

## 6. App install validation pipeline **[v0.3 baseline]**

```text
1.  Parse package file list (reject unexpected paths).
2.  Validate manifest JSON schema (schemas/manifest.schema.json).
3.  Validate resourceBudget against schemas/resource-budget.schema.json.
4.  Validate networkPolicy against schemas/network-policy.schema.json.
5.  Verify app id format, storagePrefix == "<id>:", and dataVersion >= 1.
6.  Verify entry == "index.html".
7.  Run static HTML/CSS/JS policy checks.
8.  Verify declared permissions cover bridge usage observed in static analysis.
9.  Verify required capabilities are known on the target platform (docs/26).
10. Verify network requests in code are only through AppRuntime.call("network.request", ...).
11. Run accessibility audit (docs/23).
12. Run resource-budget pre-audit (package + file size).
13. Canonicalize package (docs/17 §6) and calculate hashes.
14. Generate signature + install report.
15. Install as immutable version inside one DB transaction (docs/27 §6).
16. Run smoke/micro tests before enabling.
17. Activate via apps.active_install_id, or quarantine on failure.
```

## 7. Smoke test format **[v0.1]**

`smoke-tests.json` lives inside the package and is the minimal contract that an app proves about itself at install time. Each entry:

```json
[
  {
    "name": "creates a note",
    "steps": [
      { "type": "click", "selector": "#new-note" },
      { "type": "fill", "selector": "#note-title", "value": "Hello" },
      { "type": "click", "selector": "#save-note" }
    ],
    "expected": {
      "textIncludes": "Hello",
      "bridgeCallsInclude": ["storage.set", "notification.toast"]
    }
  }
]
```

Step vocabulary is a strict subset of the micro-test vocabulary in docs/15 so that the same runner can execute both. Selectors must prefer `data-testid` (see docs/15).

Relationship to `tests/micro/*.microtest.json`:

- `smoke-tests.json` is **package-bundled**, runs **at install time**, and exists to gate activation of a new version. It must pass on the fake host.
- `*.microtest.json` is **platform-bundled** under `tests/micro/`, runs **after install** on real hosts under Codex control, and exists to verify cross-platform behavior. It can call mocks, advance fake timers, and reset state — capabilities not available to bundled smoke tests.

A bundled smoke test must not require mocks, timer advancement, or DB assertions. If those are needed, write a micro-test instead.

## 8. Runtime version compatibility **[v0.1]**

Semver rule applied at mount:

```text
Let R = runtime version (major.minor.patch).
Let A = manifest.runtimeVersion (major.minor.patch).

Accept A on R when:
  A.major == R.major  AND
  A.minor <= R.minor

Reject otherwise with error code `runtime_version_incompatible`.
```

Examples:

- runtime `0.1.0` accepts app `0.1.0`, `0.1.5`.
- runtime `0.1.0` rejects app `0.2.0`, `1.0.0`.
- runtime `0.2.3` accepts app `0.1.7` (downgrade-compatible) and `0.2.0`.
- runtime `0.2.3` rejects app `0.3.0`.

A dev override (`--allow-runtime-mismatch`) may be passed to the fake host and to dev native builds, but it must be refused by production builds and logged. Pre-1.0 majors follow semver semantics where 0.x is treated as the major; this matches the existing repo convention.

## 9. Recommended AI output format **[v0.1]**

AI generation should return one JSON object containing files:

```json
{
  "manifest": { /* see §2 */ },
  "files": [
    { "path": "manifest.json", "content": "..." },
    { "path": "index.html", "content": "..." },
    { "path": "styles.css", "content": "..." },
    { "path": "app.js", "content": "..." }
  ],
  "smokeTests": [],
  "migrations": []
}
```

AI must **not** generate `signature.json`, `install-report.json`, or `content-hashes.json`. The platform produces those after canonicalization.

## 10. Database-backed package persistence **[v0.4]**

After validation, canonicalization, and signing, every installable package is persisted as an immutable app version inside one DB transaction:

```text
manifest.json        -> app_versions.manifest_json
index.html           -> app_files(path='index.html')
styles.css           -> app_files(path='styles.css')
app.js               -> app_files(path='app.js')
smoke-tests.json     -> app_files(path='smoke-tests.json')
migrations/*.json    -> app_migrations + app_files
manifest.permissions -> app_permissions per install_id
install diagnostics  -> app_install_reports
```

Version activation happens through `apps.active_install_id`. Files are reconstructed from `app_files` for runtime mount. Later versions may move large assets to filesystem/object storage, but the DB remains the source of truth for metadata, hashes, permissions, and active-version pointers.

## 11. Package size limits **[v0.3]**

| Limit | Default | Override |
|---|---:|---|
| Total uncompressed package | 1 MiB | `manifest.resourceBudget.maxPackageBytes` up to 4 MiB |
| Any single file | 512 KiB | `manifest.resourceBudget.maxFileBytes` up to 2 MiB |
| Number of files | 32 | hard cap, not user-overridable |
| `migrations/` files | 16 | hard cap |

Overrides require user approval at install (`install_report.requiresUserApproval = true`).
