# Project Structure

## Recommended monorepo tree

```text
repo/
  README.md
  LICENSE
  .gitignore

  docs/
    00_PRD.md
    01_ARCHITECTURE.md
    02_PROJECT_STRUCTURE.md
    03_RUNTIME_API_SPEC.md
    04_WEBAPP_PACKAGE_SPEC.md
    05_NATIVE_PLATFORM_REQUIREMENTS.md
    06_ZIG_CORE_SPEC.md
    07_SECURITY_MODEL.md
    08_TEST_PLAN.md
    09_CODEX_IMPLEMENTATION_PLAN.md
    10_ACCEPTANCE_CHECKLIST.md
    11_AI_GENERATION_PROMPTS.md
    12_RELEASE_AND_CI.md

  schemas/
    manifest.schema.json
    app-package.schema.json
    bridge-request.schema.json
    bridge-response.schema.json
    core-step.schema.json

  zig-core/
    build.zig
    build.zig.zon
    include/
      zig_core.h
    src/
      lib.zig
      ffi.zig
      core.zig
      event.zig
      action.zig
      storage_model.zig
      codec.zig
      replay.zig
    tests/
      core_tests.zig
      replay_tests.zig
      fixture_tests.zig

  runtime-web/
    package.json                    # dev-only for tests/tools, not needed at app runtime
    index.html
    src/
      runtime.js
      bridge.js
      app-registry.js
      sandbox-manager.js
      permission-manager.js
      quota-manager.js
      validators.js
      csp.js
      core-client.js
      storage-client.js
      network-client.js
      debug-console.js
      components/
        app-shell.js
        app-button.js
        app-card.js
        app-dialog.js
        app-table.js
        app-toast.js
        app-empty-state.js
    tests/
      unit/
      integration/
      fixtures/

  webapps/
    examples/
      notes-lite/
      task-workbench/
      file-transformer/
      api-dashboard/
      core-replay-lab/

  native/
    ios/
      NativeAIWebappHost.xcodeproj
      NativeAIWebappHost/
        App.swift
        WebHostView.swift
        WebBridge.swift
        ZigCoreBridge.swift
        PlatformStorage.swift
        PlatformDialogs.swift
        PlatformNotifications.swift
        PlatformNetwork.swift
        Resources/
          runtime/
          examples/

    macos/
      NativeAIWebappHost.xcodeproj
      NativeAIWebappHost/
        App.swift
        WebHostView.swift
        WebBridge.swift
        ZigCoreBridge.swift
        PlatformStorage.swift
        PlatformDialogs.swift
        PlatformNotifications.swift
        PlatformNetwork.swift
        Resources/
          runtime/
          examples/

    android/
      settings.gradle.kts
      build.gradle.kts
      app/
        build.gradle.kts
        src/main/
          AndroidManifest.xml
          java/com/example/nativeaiwebapp/
            MainActivity.kt
            WebBridge.kt
            ZigCoreBridge.kt
            PlatformStorage.kt
            PlatformDialogs.kt
            PlatformNotifications.kt
            PlatformNetwork.kt
          assets/
            runtime/
            examples/
          jniLibs/
            arm64-v8a/
            x86_64/

    windows/
      NativeAIWebappHost.sln
      src/
        main.cpp
        WebViewHost.cpp
        WebBridge.cpp
        ZigCoreBridge.cpp
        PlatformStorage.cpp
        PlatformDialogs.cpp
        PlatformNotifications.cpp
        PlatformNetwork.cpp
        resources/
          runtime/
          examples/

    linux/
      meson.build
      src/
        main.c
        webkit_host.c
        web_bridge.c
        zig_core_bridge.c
        platform_storage.c
        platform_dialogs.c
        platform_notifications.c
        platform_network.c
      resources/
        runtime/
        examples/

  server/
    build.zig
    src/
      main.zig
      routes.zig
      bridge_dispatch.zig
      storage.zig
      network.zig
      core_bridge.zig
    tests/
      api_tests.zig
      contract_tests.zig

  tools/
    validate-webapp-package/
      main.js
    package-examples/
      main.js
    generate-fixtures/
      main.js
    replay-core-events/
      main.zig

  tests/
    fixtures/
      bridge/
      core/
      webapps/
    e2e/
      playwright/
      platform-smoke/
    security/
      malicious-packages/
    performance/
      bridge-latency/
      virtual-list/

  codex/
    CODEX_MASTER_PROMPT.md
    MILESTONE_TASKS.md
    PLATFORM_BOOTSTRAP_TASKS.md
    IMPLEMENTATION_GUARDRAILS.md
```

## Project split rules

### Native shells

Native shells should not contain business logic. They should expose platform services through bridge dispatchers.

### Runtime web

The runtime web layer is shared across all native shells. It should not depend on platform-specific JavaScript APIs except through the injected bridge.

### Webapps

Generated webapps are content packages. They must not import random libraries or require a build step.

### Zig core

Zig core must have no knowledge of WebView, UI, app lifecycle, platform file dialogs, push notifications, or OS storage paths. It only processes events and returns actions.

## First implementation order

1. Create Zig core with fake deterministic `core.step` behavior.
2. Create runtime web launcher that can load example apps from static files.
3. Create in-browser mock `AppRuntime.call` for local dev.
4. Implement native bridge on one easiest platform first.
5. Port the same bridge contract to the remaining platforms.
6. Replace fake core with real Zig core library calls.
7. Add validators, permissions, quotas, and tests.

## v0.3 added directories

```text
schemas/
  app-signature.schema.json
  app-migration.schema.json
  runtime-capabilities.schema.json
  runtime-snapshot.schema.json
  network-policy.schema.json
  resource-budget.schema.json
  install-report.schema.json
  app-version-record.schema.json
  accessibility-report.schema.json

tests/
  golden/
  mutation/
  accessibility/

codex/
  CODEX_TRUST_REPAIR_PROMPT.md
```

Implementation should keep the reference-host package validator and app registry in a shared module so native hosts and server cannot drift from the reference behavior.

## v0.4 persistence additions

Add these directories/files to the monorepo:

```text
repo/
  db/
    sqlite/
      001_initial.sql
      002_runtime_debug.sql
      003_codex_control.sql
      004_migrations_and_snapshots.sql
    postgres/
      001_initial.sql
      002_runtime_debug.sql
      003_codex_control.sql
      004_migrations_and_snapshots.sql

  docs/
    27_DATABASE_SCHEMA.md
    28_STORAGE_AND_MIGRATIONS.md
    29_BACKUP_EXPORT_IMPORT.md
    30_DATABASE_TEST_PLAN.md
    31_V0_4_INTEGRATION_MAP.md

  schemas/
    db-app-records.schema.json
    db-runtime-records.schema.json
    db-test-records.schema.json
    backup-export.schema.json

  runtime-web/src/
    database-backed-storage-client.js     # host-facing interface; generated apps still use storage.* only
    app-install-repository.js
    runtime-log-repository.js

  native/*/
    PlatformDatabase.*                    # SQLite open/migrate/transaction layer
    PlatformStorage.*                     # app_storage bridge implementation
    PackageRegistry.*                     # apps/app_versions/app_files/app_permissions

  server/src/
    db.zig                               # SQLite/Postgres adapter boundary
    repositories.zig
    backup_export.zig

  tools/
    db-migrate/
    db-export/
    db-import/

  tests/
    db/
    fixtures/db/
```
