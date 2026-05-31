import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const windowsDir = path.join(repoRoot, "native", "windows");

function commandWorks(command, args = ["--version"]) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test(
  "Windows WebView2 host builds and optionally runs native smoke",
  {
    skip: process.platform !== "win32"
      ? "Windows native smoke only runs on Windows hosts"
      : !commandWorks("cmake")
        ? "cmake is not available"
        : !commandWorks("zig", ["version"])
          ? "zig is not available"
          : false,
    timeout: 180_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-windows-smoke-"));
    try {
      const zigCoreDll = path.join(scratch, "zig_core.dll");
      execFileSync(
        "zig",
        [
          "build-lib",
          "src/lib.zig",
          "--name",
          "zig_core",
          "-dynamic",
          "-lc",
          `-femit-bin=${zigCoreDll}`,
        ],
        {
          cwd: path.join(repoRoot, "zig-core"),
          env: {
            ...process.env,
            ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
            ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
          },
          stdio: "ignore",
        },
      );
      assert.equal(fs.existsSync(zigCoreDll), true);

      const buildDir = path.join(scratch, "build");
      execFileSync("cmake", ["-S", windowsDir, "-B", buildDir, `-DNATIVE_AI_ZIG_CORE_DLL=${zigCoreDll}`], { stdio: "ignore" });
      execFileSync("cmake", ["--build", buildDir, "--config", "Debug"], { stdio: "ignore" });
      const binaryPath = resolveWindowsHostBinary(buildDir);
      assert.notEqual(binaryPath, null, "NativeAIWebappHost.exe should exist after CMake build");
      const binaryDir = path.dirname(binaryPath);
      assert.equal(
        fs.existsSync(path.join(binaryDir, "zig_core.dll")),
        true,
        "zig_core.dll should be staged next to NativeAIWebappHost.exe for package-style loading",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "runtime", "index.html")),
        true,
        "runtime-web should be staged under the WebView2 /runtime resource path",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "webapps", "examples", "notes-lite", "manifest.json")),
        true,
        "example apps should be staged under the WebView2 /webapps/examples resource path",
      );
      assert.equal(
        fs.existsSync(path.join(binaryDir, "resources", "db", "sqlite", "001_initial.sql")),
        true,
        "SQLite migrations should be staged under packaged resources",
      );

      runOptionalSmoke({ binaryPath, scratch });
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Windows release host rejects dev-only startup flags and audits the rejection",
  {
    skip: process.platform !== "win32"
      ? "Windows native smoke only runs on Windows hosts"
      : !commandWorks("cmake")
        ? "cmake is not available"
        : !commandWorks("zig", ["version"])
          ? "zig is not available"
          : false,
    timeout: 180_000,
  },
  () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-windows-production-guard-"));
    try {
      const buildDir = path.join(scratch, "release-build");
      execFileSync("cmake", ["-S", windowsDir, "-B", buildDir], { stdio: "ignore" });
      execFileSync("cmake", ["--build", buildDir, "--config", "Release"], { stdio: "ignore" });
      const binaryPath = resolveWindowsHostBinary(buildDir);
      assert.notEqual(binaryPath, null, "NativeAIWebappHost.exe should exist after Release CMake build");

      const dataHome = path.join(scratch, "data-home");
      const forbiddenFlags = [
        "--native-ai-dev-control",
        "--allow-unsigned-dev",
        "--allow-runtime-mismatch=1",
        "--control-plane-port=5123",
      ];
      for (const flag of forbiddenFlags) {
        const result = spawnSync(binaryPath, [flag], {
          env: {
            ...process.env,
            NATIVE_AI_WINDOWS_SMOKE_DATA_HOME: dataHome,
          },
          cwd: path.dirname(binaryPath),
          encoding: "utf8",
          timeout: 30_000,
        });
        const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
        assert.equal(result.error, undefined, output);
        assert.equal(result.status, 1, output);
      }

      const dbPath = path.join(dataHome, "NativeAIWebappPlatform", "platform.sqlite");
      assert.equal(fs.existsSync(dbPath), true, "production guard should create the platform audit database");
      const databaseBytes = fs.readFileSync(dbPath, "utf8");
      assert.match(databaseBytes, /native\.production_guard/);
      assert.match(databaseBytes, /dev_only_flag/);
      for (const flag of forbiddenFlags) {
        assert.equal(databaseBytes.includes(flag), true, `audit database should include rejected flag ${flag}`);
      }
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

test(
  "Windows debug dev control health is token-gated and audited",
  {
    skip: process.platform !== "win32"
      ? "Windows native smoke only runs on Windows hosts"
      : !commandWorks("cmake")
        ? "cmake is not available"
        : false,
    timeout: 180_000,
  },
  async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-windows-dev-control-"));
    let child = null;
    try {
      const zigCoreDll = path.join(scratch, "zig_core.dll");
      execFileSync(
        "zig",
        [
          "build-lib",
          "src/lib.zig",
          "--name",
          "zig_core",
          "-dynamic",
          "-lc",
          `-femit-bin=${zigCoreDll}`,
        ],
        {
          cwd: path.join(repoRoot, "zig-core"),
          env: {
            ...process.env,
            ZIG_GLOBAL_CACHE_DIR: path.join(scratch, "zig-global-cache"),
            ZIG_LOCAL_CACHE_DIR: path.join(scratch, "zig-local-cache"),
          },
          stdio: "ignore",
        },
      );
      const buildDir = path.join(scratch, "debug-build");
      execFileSync("cmake", ["-S", windowsDir, "-B", buildDir, `-DNATIVE_AI_ZIG_CORE_DLL=${zigCoreDll}`], { stdio: "ignore" });
      execFileSync("cmake", ["--build", buildDir, "--config", "Debug"], { stdio: "ignore" });
      const binaryPath = resolveWindowsHostBinary(buildDir);
      assert.notEqual(binaryPath, null, "NativeAIWebappHost.exe should exist after Debug CMake build");

      const dataHome = path.join(scratch, "data-home");
      const resultFile = path.join(scratch, "dev-control-result.txt");
      const tokenPath = path.join(scratch, "control.token");
      child = spawn(binaryPath, ["--native-ai-dev-control", "--control-plane-port=0"], {
        env: {
          ...process.env,
          NATIVE_AI_WINDOWS_SMOKE_DATA_HOME: dataHome,
          NATIVE_AI_WINDOWS_SMOKE_RESULT_FILE: resultFile,
          PLATFORM_CONTROL_TOKEN_FILE: tokenPath,
        },
        cwd: path.dirname(binaryPath),
        encoding: "utf8",
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      });

      const ready = await waitForWindowsControlReady(child, resultFile);
      assert.equal(ready.tokenPath, tokenPath);
      const token = fs.readFileSync(tokenPath, "utf8").trim();
      assert.match(token, /^[A-Za-z0-9_-]{43}$/);

      const unauthorized = await requestControlHealth(ready.port);
      assert.equal(unauthorized.statusCode, 401);
      assert.equal(JSON.parse(unauthorized.body).error.code, "control_auth_required");

      const authorized = await requestControlHealth(ready.port, token);
      assert.equal(authorized.statusCode, 200, authorized.body);
      const body = JSON.parse(authorized.body);
      assert.equal(body.ok, true);
      assert.equal(body.target, "windows");
      assert.equal(body.controlPlane.port, ready.port);

      const unauthorizedSession = await requestControl(ready.port, "/sessions", {
        method: "POST",
        body: { appId: "notes-lite" },
      });
      assert.equal(unauthorizedSession.statusCode, 401);
      assert.equal(JSON.parse(unauthorizedSession.body).error.code, "control_auth_required");

      const session = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "task-workbench", metadata: { smoke: "windows-dev-control" } },
      });
      assert.equal(session.statusCode, 200, session.body);
      const sessionBody = JSON.parse(session.body);
      assert.equal(sessionBody.ok, true);
      assert.match(sessionBody.result.controlSessionId, /^control-/);
      assert.match(sessionBody.result.runtimeSessionId, /^session-/);
      assert.equal(sessionBody.result.appId, "task-workbench");
      const sessionId = sessionBody.result.controlSessionId;

      const snapshot = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(sessionId)}/snapshot`, { token });
      assert.equal(snapshot.statusCode, 200, snapshot.body);
      assert.equal(JSON.parse(snapshot.body).result.snapshot.target, "windows");

      const events = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/events`, { token });
      assert.equal(events.statusCode, 200, events.body);
      assert.equal(Array.isArray(JSON.parse(events.body).result.bridgeCalls), true);

      const capabilities = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/capabilities`, { token });
      assert.equal(capabilities.statusCode, 200, capabilities.body);
      assert.equal(JSON.parse(capabilities.body).result.platform, "windows");

      const command = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.health", args: {} },
      });
      assert.equal(command.statusCode, 200, command.body);
      assert.equal(JSON.parse(command.body).result.target, "windows");

      const commandCapabilities = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.capabilities", args: {} },
      });
      assert.equal(commandCapabilities.statusCode, 200, commandCapabilities.body);
      assert.equal(JSON.parse(commandCapabilities.body).result.features["runtime.capabilities"], true);

      const apiSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "api-dashboard", metadata: { smoke: "windows-network-mock" } },
      });
      assert.equal(apiSession.statusCode, 200, apiSession.body);
      const apiSessionId = JSON.parse(apiSession.body).result.controlSessionId;

      const networkMock = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.network_mock_set",
          args: {
            appId: "api-dashboard",
            method: "GET",
            urlPattern: "https://api.example.com/status",
            response: {
              status: 200,
              headers: { "content-type": "application/json" },
              bodyText: "{\"ok\":true,\"source\":\"windows-network-mock\"}",
              delayMs: 1,
            },
          },
        },
      });
      assert.equal(networkMock.statusCode, 200, networkMock.body);
      assert.match(JSON.parse(networkMock.body).result.mockId, /^netmock-/);

      const mockedNetwork = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "api-dashboard",
            method: "network.request",
            params: { url: "https://api.example.com/status", method: "GET", headers: {} },
          },
        },
      });
      assert.equal(mockedNetwork.statusCode, 200, mockedNetwork.body);
      assert.equal(JSON.parse(mockedNetwork.body).result.result.bodyText.includes("windows-network-mock"), true);

      const resetNetworkMocks = await requestControl(ready.port, `/sessions/${encodeURIComponent(apiSessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.network_mock_reset", args: { appId: "api-dashboard" } },
      });
      assert.equal(resetNetworkMocks.statusCode, 200, resetNetworkMocks.body);
      assert.equal(Number(JSON.parse(resetNetworkMocks.body).result.cleared) >= 1, true);

      const fileSession = await requestControl(ready.port, "/control/sessions", {
        method: "POST",
        token,
        body: { appId: "file-transformer", metadata: { smoke: "windows-dialog-mock" } },
      });
      assert.equal(fileSession.statusCode, 200, fileSession.body);
      const fileSessionId = JSON.parse(fileSession.body).result.controlSessionId;

      const dialogMock = await requestControl(ready.port, `/sessions/${encodeURIComponent(fileSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.dialog_mock_set",
          args: {
            appId: "file-transformer",
            method: "dialog.openFile",
            response: {
              files: [{ name: "windows-mock.txt", mime: "text/plain", size: 5, text: "hello" }],
              cancelled: false,
            },
          },
        },
      });
      assert.equal(dialogMock.statusCode, 200, dialogMock.body);
      assert.match(JSON.parse(dialogMock.body).result.mockId, /^dialogmock-/);

      const mockedDialog = await requestControl(ready.port, `/sessions/${encodeURIComponent(fileSessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "file-transformer",
            method: "dialog.openFile",
            params: { accept: ["text/plain"] },
          },
        },
      });
      assert.equal(mockedDialog.statusCode, 200, mockedDialog.body);
      assert.equal(JSON.parse(mockedDialog.body).result.result.files[0].text, "hello");

      const callBridge = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "storage.set",
            params: {
              key: "task-workbench:windows-dev-control-key",
              value: { source: "windows-dev-control" },
            },
          },
        },
      });
      assert.equal(callBridge.statusCode, 200, callBridge.body);
      const callBridgeBody = JSON.parse(callBridge.body);
      assert.equal(callBridgeBody.result.id, "control_call_bridge");
      assert.equal(callBridgeBody.result.ok, true);

      const appLog = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "app.log",
            params: {
              level: "info",
              message: "Windows control log probe",
            },
          },
        },
      });
      assert.equal(appLog.statusCode, 200, appLog.body);
      assert.equal(JSON.parse(appLog.body).result.ok, true);

      const notificationToast = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.call_bridge",
          args: {
            appId: "task-workbench",
            method: "notification.toast",
            params: {
              level: "success",
              message: "Windows control saved",
            },
          },
        },
      });
      assert.equal(notificationToast.statusCode, 200, notificationToast.body);
      assert.equal(JSON.parse(notificationToast.body).result.ok, true);

      const coreStep = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.core_step",
          args: {
            appId: "task-workbench",
            event: { type: "CreateTask", payload: { title: "Windows control task" } },
          },
        },
      });
      assert.equal(coreStep.statusCode, 200, coreStep.body);
      const coreStepBody = JSON.parse(coreStep.body);
      assert.equal(coreStepBody.result.id, "control_core_step");
      assert.equal(coreStepBody.result.ok, true);

      const resourceUsage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.resource_usage", args: { appId: "task-workbench" } },
      });
      assert.equal(resourceUsage.statusCode, 200, resourceUsage.body);
      const resourceUsageBody = JSON.parse(resourceUsage.body);
      assert.equal(resourceUsageBody.result.appId, "task-workbench");
      assert.equal(Number(resourceUsageBody.result.bridgeCalls) >= 2, true);
      assert.equal(Number(resourceUsageBody.result.coreEvents) >= 1, true);

      const eventLog = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.event_log", args: { appId: "task-workbench" } },
      });
      assert.equal(eventLog.statusCode, 200, eventLog.body);
      assert.equal(JSON.parse(eventLog.body).result.bridgeCalls.some((row) => row.method === "storage.set"), true);
      assert.equal(JSON.parse(eventLog.body).result.coreEvents.length >= 1, true);

      const consoleLogs = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.console_logs", args: { appId: "task-workbench" } },
      });
      assert.equal(consoleLogs.statusCode, 200, consoleLogs.body);
      assert.equal(
        JSON.parse(consoleLogs.body).result.logs.some((row) => row.params?.message === "Windows control log probe"),
        true,
      );

      const runtimeBridgeCalls = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.bridge_calls", args: { appId: "task-workbench" } },
      });
      assert.equal(runtimeBridgeCalls.statusCode, 200, runtimeBridgeCalls.body);
      assert.equal(JSON.parse(runtimeBridgeCalls.body).result.some((row) => row.method === "storage.set"), true);
      assert.equal(JSON.parse(runtimeBridgeCalls.body).result.some((row) => row.method === "notification.toast"), true);

      const bridgeCallAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_bridge_call", args: { appId: "task-workbench", method: "notification.toast" } },
      });
      assert.equal(bridgeCallAssert.statusCode, 200, bridgeCallAssert.body);
      assert.equal(JSON.parse(bridgeCallAssert.body).result.method, "notification.toast");

      const noConsoleErrors = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.assert_no_console_errors", args: { appId: "task-workbench" } },
      });
      assert.equal(noConsoleErrors.statusCode, 200, noConsoleErrors.body);
      assert.equal(JSON.parse(noConsoleErrors.body).result.errors, 0);

      const notificationCapture = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.notification_capture", args: { appId: "task-workbench" } },
      });
      assert.equal(notificationCapture.statusCode, 200, notificationCapture.body);
      assert.equal(
        JSON.parse(notificationCapture.body).result.notifications.some((row) => row.message === "Windows control saved"),
        true,
      );

      const missingResourceAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.resource_usage", args: {} },
      });
      assert.equal(missingResourceAppId.statusCode, 400);
      assert.equal(JSON.parse(missingResourceAppId.body).error.code, "invalid_request");

      const dbSnapshot = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.snapshot", args: {} },
      });
      assert.equal(dbSnapshot.statusCode, 200, dbSnapshot.body);
      assert.equal(Array.isArray(JSON.parse(dbSnapshot.body).result.apps), true);

      const dbStorage = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: { appId: "task-workbench" } },
      });
      assert.equal(dbStorage.statusCode, 200, dbStorage.body);
      assert.equal(
        JSON.parse(dbStorage.body).result.rows.some((row) => row.key === "task-workbench:windows-dev-control-key"),
        true,
      );

      const controlStorageSet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_set",
          args: {
            appId: "task-workbench",
            key: "task-workbench:windows-control-storage",
            value: { source: "runtime.storage_set" },
          },
        },
      });
      assert.equal(controlStorageSet.statusCode, 200, controlStorageSet.body);
      assert.equal(JSON.parse(controlStorageSet.body).result.ok, true);

      const controlStorageGet = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.storage_get",
          args: {
            appId: "task-workbench",
            key: "task-workbench:windows-control-storage",
          },
        },
      });
      assert.equal(controlStorageGet.statusCode, 200, controlStorageGet.body);
      assert.equal(JSON.parse(controlStorageGet.body).result.result.value.source, "runtime.storage_set");

      const controlStorageAssert = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: {
          tool: "runtime.assert_storage",
          args: {
            appId: "task-workbench",
            key: "task-workbench:windows-control-storage",
            value: { source: "runtime.storage_set" },
          },
        },
      });
      assert.equal(controlStorageAssert.statusCode, 200, controlStorageAssert.body);
      assert.equal(JSON.parse(controlStorageAssert.body).result.ok, true);

      const missingDbAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: {} },
      });
      assert.equal(missingDbAppId.statusCode, 400);
      assert.equal(JSON.parse(missingDbAppId.body).error.code, "invalid_request");

      const unsafeDbTool = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_sql", args: { sql: "SELECT * FROM apps" } },
      });
      assert.equal(unsafeDbTool.statusCode, 400);
      assert.equal(JSON.parse(unsafeDbTool.body).error.code, "unsupported_tool");

      for (const tool of ["db.query_app_versions", "db.query_bridge_calls", "db.query_core_events", "db.query_test_runs"]) {
        const response = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
          method: "POST",
          token,
          body: { tool, args: { appId: "task-workbench" } },
        });
        assert.equal(response.statusCode, 200, response.body);
        assert.equal(Array.isArray(JSON.parse(response.body).result.rows), true);
      }

      const debugBundle = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.export_debug_bundle", args: {} },
      });
      assert.equal(debugBundle.statusCode, 200, debugBundle.body);
      const debugBundleBody = JSON.parse(debugBundle.body);
      assert.equal(debugBundleBody.result.type, "debug-bundle");
      assert.equal(debugBundleBody.result.source.platform, "windows");
      assert.equal(debugBundleBody.result.source.target, "windows");
      assert.match(debugBundleBody.result.contentHash, /^sha256:[a-f0-9]{64}$/);
      assert.equal(Array.isArray(debugBundleBody.result.apps), true);
      assert.equal(Array.isArray(debugBundleBody.result.appStorage), true);
      assert.equal(typeof debugBundleBody.result.runtimeCapabilities, "object");
      assert.equal(Array.isArray(debugBundleBody.result.debug.runtimeSessions), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.bridgeCalls), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.controlCommands), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.coreEvents), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.coreActions), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.runtimeSnapshots), true);
      assert.equal(Array.isArray(debugBundleBody.result.debug.testRuns), true);
      assert.equal(debugBundleBody.result.debug.bridgeCalls.some((row) => row.method === "storage.set"), true);
      assert.equal(debugBundleBody.result.debug.coreEvents.length >= 1, true);
      assert.equal(debugBundleBody.result.debug.coreActions.length >= 1, true);

      const backup = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.export_backup", args: {} },
      });
      assert.equal(backup.statusCode, 200, backup.body);
      const backupBody = JSON.parse(backup.body);
      assert.equal(backupBody.result.type, "backup");
      assert.equal(backupBody.result.source.platform, "windows");
      assert.match(backupBody.result.contentHash, /^sha256:[a-f0-9]{64}$/);
      assert.equal(Array.isArray(backupBody.result.apps), true);
      assert.equal(Array.isArray(backupBody.result.appVersions), true);
      assert.equal(Array.isArray(backupBody.result.appFiles), true);
      assert.equal(Array.isArray(backupBody.result.appPermissions), true);
      assert.equal(
        backupBody.result.appStorage.some((row) => row.key === "task-workbench:windows-dev-control-key"),
        true,
      );

      const importBackup = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.import_backup", args: { backup: backupBody.result } },
      });
      assert.equal(importBackup.statusCode, 200, importBackup.body);
      const importBackupBody = JSON.parse(importBackup.body);
      assert.equal(importBackupBody.result.ok, true);
      assert.equal(Number(importBackupBody.result.appStorage) >= 1, true);

      const resetWithoutConfirm = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "platform.reset_webapp", args: { appId: "task-workbench" } },
      });
      assert.equal(resetWithoutConfirm.statusCode, 400);
      assert.equal(JSON.parse(resetWithoutConfirm.body).error.code, "confirmation_required");

      const storageReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.storage_reset", args: { appId: "task-workbench", confirm: true } },
      });
      assert.equal(storageReset.statusCode, 200, storageReset.body);
      const storageResetBody = JSON.parse(storageReset.body);
      assert.equal(storageResetBody.result.ok, true);
      assert.match(storageResetBody.result.snapshotId, /^snapshot-/);
      assert.equal(Number(storageResetBody.result.clearedStorageKeys) >= 1, true);

      const storageAfterReset = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "db.query_app_storage", args: { appId: "task-workbench" } },
      });
      assert.equal(storageAfterReset.statusCode, 200, storageAfterReset.body);
      assert.deepEqual(JSON.parse(storageAfterReset.body).result.rows, []);

      const clearedLogs = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.clear_logs", args: { appId: "task-workbench" } },
      });
      assert.equal(clearedLogs.statusCode, 200, clearedLogs.body);
      const clearedLogsBody = JSON.parse(clearedLogs.body);
      assert.equal(clearedLogsBody.result.ok, true);
      assert.equal(Number(clearedLogsBody.result.bridgeCallsCleared) >= 5, true);
      assert.equal(Number(clearedLogsBody.result.coreActionsCleared) >= 1, true);
      assert.equal(Number(clearedLogsBody.result.coreEventsCleared) >= 1, true);

      const bridgeCallsAfterClear = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.bridge_calls", args: { appId: "task-workbench" } },
      });
      assert.equal(bridgeCallsAfterClear.statusCode, 200, bridgeCallsAfterClear.body);
      assert.deepEqual(JSON.parse(bridgeCallsAfterClear.body).result, []);

      const missingAppId = await requestControl(ready.port, `/sessions/${encodeURIComponent(sessionId)}/command`, {
        method: "POST",
        token,
        body: { tool: "runtime.call_bridge", args: { method: "storage.set", params: {} } },
      });
      assert.equal(missingAppId.statusCode, 400);
      assert.equal(JSON.parse(missingAppId.body).error.code, "invalid_request");

      const ended = await requestControl(ready.port, `/control/sessions/${encodeURIComponent(sessionId)}`, {
        method: "DELETE",
        token,
      });
      assert.equal(ended.statusCode, 200, ended.body);
      assert.equal(JSON.parse(ended.body).result.status, "ended");

      const dbPath = path.join(dataHome, "NativeAIWebappPlatform", "platform.sqlite");
      assert.equal(fs.existsSync(dbPath), true, "dev control should create the platform audit database");
      const { DatabaseSync } = await import("node:sqlite");
      const database = new DatabaseSync(dbPath);
      try {
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_sessions WHERE target = 'windows' AND token_hash IS NOT NULL").get().count) >= 1,
          true,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE http_method = 'GET' AND path = '/health' AND decision = 'rejected' AND error_code = 'control_auth_required'").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'platform.health' AND http_method = 'GET' AND path = '/health' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.call_bridge' AND decision = 'accepted' AND error_code IS NULL").get().count),
          5,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.capabilities' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.core_step' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.resource_usage' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.event_log' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.console_logs' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.bridge_calls' AND decision = 'accepted' AND error_code IS NULL").get().count),
          2,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.assert_bridge_call' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.assert_no_console_errors' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.notification_capture' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.clear_logs' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.storage_set' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.storage_get' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'platform.reset_webapp' AND decision = 'rejected' AND error_code = 'confirmation_required'").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.storage_reset' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.assert_storage' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.network_mock_set' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.network_mock_reset' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'runtime.dialog_mock_set' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'db.snapshot' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'db.query_app_storage' AND decision = 'accepted' AND error_code IS NULL").get().count),
          2,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'db.export_debug_bundle' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'db.export_backup' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM control_commands WHERE tool = 'db.import_backup' AND decision = 'accepted' AND error_code IS NULL").get().count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM backup_exports WHERE type = 'debug-bundle' AND source_platform = 'windows' AND content_hash = ?").get(debugBundleBody.result.contentHash).count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM backup_exports WHERE type = 'backup' AND source_platform = 'windows' AND content_hash = ?").get(backupBody.result.contentHash).count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM backup_exports WHERE type = 'import' AND content_hash = ? AND imported_at IS NOT NULL").get(backupBody.result.contentHash).count),
          1,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM bridge_calls WHERE app_id = 'task-workbench'").get().count),
          0,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM app_storage WHERE app_id = 'task-workbench'").get().count),
          0,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM runtime_snapshots WHERE app_id = 'task-workbench' AND type = 'manual'").get().count) >= 1,
          true,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM core_events WHERE app_id = 'task-workbench'").get().count),
          0,
        );
        assert.equal(
          Number(database.prepare("SELECT COUNT(*) AS count FROM core_actions WHERE app_id = 'task-workbench'").get().count),
          0,
        );
      } finally {
        database.close();
      }
    } finally {
      if (child !== null) {
        await stopChild(child);
      }
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  },
);

function resolveWindowsHostBinary(buildDir) {
  for (const candidate of [
    path.join(buildDir, "Debug", "NativeAIWebappHost.exe"),
    path.join(buildDir, "NativeAIWebappHost.exe"),
    path.join(buildDir, "Release", "NativeAIWebappHost.exe"),
  ]) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function runOptionalSmoke({ binaryPath, scratch }) {
  if (process.env.NATIVE_AI_WINDOWS_SMOKE_LAUNCH !== "1") return;
  const storageKey = `notes-lite:windows-smoke-${process.pid}-${Date.now()}`;
  const storageValue = `windows-smoke-${process.pid}-${Date.now()}`;
  const dataHome = path.join(scratch, "data-home");
  const resultFile = path.join(scratch, "smoke-result.txt");
  const { NATIVE_AI_ZIG_CORE_DLL: _ignoredZigCoreDll, ...smokeEnv } = process.env;
  const baseEnv = {
    ...smokeEnv,
    NATIVE_AI_WINDOWS_SMOKE_DATA_HOME: dataHome,
    NATIVE_AI_WINDOWS_SMOKE_EXIT_AFTER: "1",
    NATIVE_AI_WINDOWS_SMOKE_RESULT_FILE: resultFile,
  };

  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_RUNTIME_LOADED", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "runtime-load",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "storage-set",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "core-step",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_FIXED_BRIDGE_SURFACE_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "fixed-bridge-surface",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_SET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-storage-set",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY: storageKey,
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_BRIDGE_CORE_STEP_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "bridge-core-step",
  });
  runSmoke(binaryPath, resultFile, "NATIVE_AI_WINDOWS_SMOKE_RUNTIME_APP_STORAGE_GET_OK", {
    ...baseEnv,
    NATIVE_AI_WINDOWS_SMOKE: "runtime-app-storage-get",
    NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE: storageValue,
  });
}

function runSmoke(binaryPath, resultFile, marker, env) {
  fs.rmSync(resultFile, { force: true });
  const result = spawnSync(binaryPath, [], { env, cwd: path.dirname(binaryPath), encoding: "utf8", timeout: 30_000 });
  const markerOutput = fs.existsSync(resultFile) ? fs.readFileSync(resultFile, "utf8") : "";
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}\n${markerOutput}`;
  assert.equal(output.includes("NATIVE_AI_WINDOWS_SMOKE_FAILED"), false, output);
  assert.equal(result.error, undefined, output);
  assert.equal(result.status, 0, output);
  assert.equal(output.includes(marker), true, `Timed out waiting for ${marker}\n${output}`);
}

function waitForWindowsControlReady(child, resultFile) {
  return new Promise((resolve, reject) => {
    let settled = false;
    let output = "";
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        reject(new Error(`Timed out waiting for Windows dev control readiness\n${output}`));
      }
    }, 30_000);

    function completeFromText(text) {
      output += text;
      const match = output.match(/NATIVE_AI_WINDOWS_CONTROL_READY port=(\d+) token_path=([^\r\n]+)/);
      if (!match || settled) return;
      settled = true;
      clearTimeout(timer);
      clearInterval(poll);
      resolve({ port: Number(match[1]), tokenPath: match[2], output });
    }

    const poll = setInterval(() => {
      if (fs.existsSync(resultFile)) {
        completeFromText(fs.readFileSync(resultFile, "utf8"));
      }
    }, 100);
    child.stdout.on("data", (chunk) => completeFromText(chunk.toString("utf8")));
    child.stderr.on("data", (chunk) => completeFromText(chunk.toString("utf8")));
    child.once("error", (error) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        clearInterval(poll);
        reject(error);
      }
    });
    child.once("exit", (code, signal) => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        clearInterval(poll);
        reject(new Error(`Windows host exited before dev control was ready code=${code} signal=${signal}\n${output}`));
      }
    });
  });
}

function requestControlHealth(port, token = null) {
  return requestControl(port, "/health", { token });
}

function requestControl(port, pathName, { method = "GET", token = null, body = null } = {}) {
  return new Promise((resolve, reject) => {
    const headers = token ? { "X-Platform-Control-Token": token } : {};
    let bodyText = null;
    if (body !== null) {
      bodyText = JSON.stringify(body);
      headers["content-type"] = "application/json";
      headers["content-length"] = Buffer.byteLength(bodyText);
    }
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port,
        path: pathName,
        method,
        headers,
        timeout: 10_000,
      },
      (res) => {
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () => resolve({ statusCode: res.statusCode, body }));
      },
    );
    req.on("error", reject);
    req.on("timeout", () => {
      req.destroy(new Error(`Timed out waiting for Windows dev control ${method} ${pathName}`));
    });
    req.end(bodyText);
  });
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill();
  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      if (child.exitCode === null && child.signalCode === null) {
        spawnSync("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
      }
      resolve();
    }, 5_000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}
