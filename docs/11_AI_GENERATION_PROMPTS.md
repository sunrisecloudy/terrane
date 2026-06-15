# AI Generation Prompts

## 1. Main webapp generation prompt

Use this prompt when asking an AI agent to create a build-free generated app package.

```text
You are generating a build-free mini webapp for a sandboxed native WebView runtime.

The generated app must run on iOS, Android, macOS, Windows, and Linux WebViews without a build step.

Target format:
- HTML
- CSS
- Vanilla JavaScript
- No TypeScript
- No JSX
- No npm packages
- No bundler
- No external CDN
- No remote scripts
- No eval
- No dynamic import
- No direct native/platform APIs

The app runs inside a sandboxed iframe. It may only communicate with the host through this bridge:

await AppRuntime.call(method, params)

Allowed bridge methods:
- core.step
- storage.get
- storage.set
- storage.remove
- storage.list
- dialog.openFile
- dialog.saveFile
- notification.toast
- network.request
- app.log

Do not invent new bridge methods.

The app must be generated as a package with these files:
- manifest.json
- index.html
- styles.css
- app.js
- smoke-tests.json

The manifest must include:
- id
- name
- version
- runtimeVersion
- dataVersion (positive integer)
- entry
- description
- permissions
- storagePrefix (must equal "<id>:")
- capabilities (with required and optional arrays)
- resourceBudget (all keys present; see docs/22 §2)
- networkPolicy (with allow array; empty if no network is needed)

Do NOT include the deprecated `networkAllowlist` field. The validator rejects packages that have it.

Security rules:
- All storage keys must be prefixed with the app id plus colon.
- All native/core actions must go through AppRuntime.call.
- Never access window.parent directly except through the provided bridge.
- Never use cookies, localStorage, sessionStorage, IndexedDB, direct fetch, XMLHttpRequest, or WebSocket.
- Network access must use AppRuntime.call("network.request", ...).
- Treat all bridge responses as untrusted and validate before rendering.
- Escape user-generated text before inserting into HTML.
- Prefer textContent over innerHTML.

UI rules:
- Make the UI polished, dense but readable, and usable on mobile and desktop.
- Use responsive layout.
- Support light and dark mode with CSS variables.
- Include empty states, loading states, and error states.
- Include keyboard-friendly controls where appropriate.
- Use semantic HTML.
- Keep styling self-contained in styles.css.

App behavior:
Create a webapp for the following request:

{{USER_APP_REQUEST}}

Output rules:
Return only one JSON object. No markdown. No explanations.

JSON shape:
{
  "manifest": {},
  "files": [
    { "path": "manifest.json", "content": "" },
    { "path": "index.html", "content": "" },
    { "path": "styles.css", "content": "" },
    { "path": "app.js", "content": "" },
    { "path": "smoke-tests.json", "content": "" }
  ],
  "smokeTests": [
    { "name": "", "steps": [], "expected": {} }
  ]
}
```

## 2. Repair prompt

```text
You are repairing a generated webapp package for a sandboxed runtime.

You will receive:
1. The current package files.
2. Validation errors.
3. Smoke test failures.

Fix only what is necessary.

Rules:
- Keep the package build-free.
- Do not add external dependencies.
- Do not invent bridge methods.
- Preserve app id and storage prefix unless the validation error says they are invalid.
- Return the same JSON package shape.

Validation errors:
{{VALIDATION_ERRORS}}

Current package:
{{PACKAGE_JSON}}
```

## 3. App improvement prompt

```text
Improve this generated webapp package while preserving runtime compatibility.

Goals:
{{GOALS}}

Rules:
- No build step.
- No external dependencies.
- No new bridge methods unless they are in the allowed list.
- Update smoke tests if behavior changes.
- Keep storage keys prefixed with app id.

Package:
{{PACKAGE_JSON}}
```

## 4. Codex implementation prompt

```text
Implement the Terrane v0.1 according to the documentation in docs/.

Start with the smallest vertical slice:
1. Forge core fake `core.step` with tests.
2. Web runtime launcher with browser mock bridge.
3. Load the five example apps.
4. Validate manifests and enforce permissions.
5. Add server core.step endpoint.
6. Then add native shells one platform at a time.

Do not convert generated app format to React, TypeScript, Vite, or npm packages. Generated apps must remain build-free HTML/CSS/vanilla JS packages.

Keep bridge API exactly as documented unless you update docs, schemas, examples, and tests in the same change.
```

## v0.3 prompt additions

Add this block to every app-generation prompt:

```text
The manifest must include dataVersion, capabilities, resourceBudget, and networkPolicy.

Use this default resourceBudget unless the app truly needs lower limits:
{
  "maxDomNodes": 2000,
  "maxStorageBytes": 5242880,
  "maxBridgeCallsPerMinute": 600,
  "maxNetworkRequestsPerMinute": 60,
  "maxTimers": 64,
  "maxLogLinesPerMinute": 120,
  "maxPackageBytes": 1048576,
  "maxFileBytes": 524288
}

Use networkPolicy.allow = [] unless the user explicitly asks for network access.
If network access is needed, declare exact origins, methods, headers, size limits, and timeout.

Do not generate signatures. The platform installer signs packages.
Do not change stored data shape without increasing dataVersion and adding migration files.
```

Also add this repair instruction:

```text
When fixing a failing generated app, patch the smallest set of files. If you change bridge usage, update permissions/capabilities/networkPolicy. If you change storage shape, update dataVersion and migrations. Rerun schema validation, static policy audit, accessibility audit, smoke tests, and affected micro-tests.
```
