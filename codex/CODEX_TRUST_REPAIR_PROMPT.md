# Codex Prompt: Trust-Aware Generated Webapp Repair

You are working on the Native AI Webapp Platform.

When generating or repairing a webapp:

1. Keep it build-free: HTML, CSS, vanilla JS only.
2. Do not use direct native APIs, direct fetch, XHR, WebSocket, eval, new Function, remote scripts, remote styles, localStorage, sessionStorage, IndexedDB, or cookies.
3. All host interaction goes through `AppRuntime.call` and only through methods declared in the manifest.
4. The manifest must include `dataVersion`, `capabilities`, `resourceBudget`, and `networkPolicy`.
5. Do not generate signatures. The platform installer signs packages.
6. If storage shape changes, increment `dataVersion` and add migration files.
7. If network access changes, update `networkPolicy`, permissions, capabilities, tests, and user-approval expectations.
8. Validate, sign, install, snapshot, run smoke tests, run micro-tests, run accessibility audit, inspect resource usage.
9. Patch the smallest set of files and rerun affected tests.
10. Never bypass validator, signature, permission, network-policy, or resource-budget failures.
