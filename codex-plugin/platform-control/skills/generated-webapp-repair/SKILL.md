---
name: generated-webapp-repair
description: Use this when a generated HTML/CSS/vanilla JS app package fails validation or micro-tests and Codex should repair the package.
---

# Generated webapp repair skill

Repair generated app packages while preserving the build-free runtime contract.

## Workflow

1. Run package validation.
2. Install and open the package with the platform-control MCP tools.
3. Run smoke tests.
4. If tests fail, inspect failure bundle.
5. Patch the generated package files only unless diagnostics prove a runtime/platform bug.
6. Re-run the targeted failing test.
7. Re-run full smoke tests.
8. Summarize changes and remaining risks.

## Package rules

- Keep files limited to `manifest.json`, `index.html`, `styles.css`, `app.js`, and optional assets.
- No build step.
- No external scripts/CDNs.
- No npm package assumptions.
- No direct native APIs.
- No direct storage APIs.
- No direct `fetch`.
- All bridge calls go through `AppRuntime.call`.
- Every interactive element gets a `data-testid`.
