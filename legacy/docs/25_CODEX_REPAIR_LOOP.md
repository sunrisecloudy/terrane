# Codex Repair Loop

## 1. Purpose

AI-generated apps will fail. The platform should make failures cheap, local, reproducible, and granular.

## 2. Required loop

```text
1. Generate source package.
2. Validate schema and policy.
3. Run static audits: HTML, JS, CSS, permissions, network, accessibility.
4. Sign or dev-sign package.
5. Install as immutable version.
6. Create pre-test snapshot.
7. Run smoke tests.
8. Run micro-tests for modified features.
9. Collect install report, logs, bridge calls, resource usage, accessibility report, and snapshot.
10. If failing, patch only necessary files.
11. Repeat from validation.
12. Enable only after passing gates.
```

## 3. Codex patch discipline

Codex must:

- preserve app id and storage prefix unless explicitly migrating;
- update `dataVersion` and migration files when storage shape changes;
- update permissions and network policy when bridge usage changes;
- avoid inventing bridge methods;
- prefer minimal diffs;
- rerun affected tests after every patch;
- never bypass validator failures.

## 4. Recommended MCP tool sequence

```text
platform.validate_package
platform.sign_webapp_package
platform.install_webapp_package
platform.open_webapp
runtime.capabilities
runtime.snapshot
runtime.run_smoke_tests
runtime.run_microtest
runtime.run_accessibility_audit
runtime.resource_usage
platform.create_snapshot
platform.install_report
```

For failures:

```text
runtime.console_logs
runtime.bridge_calls
runtime.event_log
runtime.storage_get
runtime.screenshot
runtime.accessibility_snapshot
platform.create_snapshot
```

Then patch and repeat.

## 5. Repair reports

Every repair run should produce:

```json
{
  "appId": "notes-lite",
  "startedAt": "...",
  "attempts": 2,
  "finalStatus": "passed",
  "changedFiles": ["app.js"],
  "testsRun": ["notes-lite-create-note"],
  "snapshots": ["snap_..."],
  "remainingWarnings": []
}
```

## 6. Stop conditions

Codex should stop and report instead of continuing if:

- the same failure persists after 3 patches;
- the required capability is absent on the platform;
- user approval is required for higher permissions;
- migration would destroy user data;
- a platform-specific WebView bug is suspected and needs manual review.

## Database-backed repair diagnostics

The repair loop should include DB state in failure diagnostics:

```text
install report
active app version
app_storage rows for the app
recent bridge_calls
recent core_events/core_actions
runtime snapshot
test_runs diagnostics
```

Codex should fetch these through safe DB tools and then patch only the generated app package/migration files needed to fix the failing behavior.
