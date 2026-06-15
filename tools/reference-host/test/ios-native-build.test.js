import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const iosDir = path.join(repoRoot, "native", "ios");
const bundleId = "dev.terrane.host.ios";
const smokeLoadedMarker = "TERRANE_IOS_SMOKE_RUNTIME_LOADED";
const smokeStorageSetMarker = "TERRANE_IOS_SMOKE_STORAGE_SET_OK";
const smokeStorageGetMarker = "TERRANE_IOS_SMOKE_STORAGE_GET_OK";
const smokeStorageResetMarker = "TERRANE_IOS_SMOKE_STORAGE_RESET_OK";
const smokeCoreStepMarker = "TERRANE_IOS_SMOKE_CORE_STEP_OK";
const smokeAllExamplesMarker = "TERRANE_IOS_SMOKE_ALL_EXAMPLES_OK";
const smokeMarkerFile = "terrane-ios-smoke-runtime-loaded.txt";
const exampleAppIds = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab"];

function commandWorks(command, args) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function simulatorSdkPath() {
  return execFileSync("xcrun", ["--sdk", "iphonesimulator", "--show-sdk-path"], {
    encoding: "utf8",
  }).trim();
}

function hasIPhoneSimulatorSdk() {
  try {
    return simulatorSdkPath().length > 0;
  } catch {
    return false;
  }
}

function deviceSdkPath() {
  return execFileSync("xcrun", ["--sdk", "iphoneos", "--show-sdk-path"], {
    encoding: "utf8",
  }).trim();
}

function hasIPhoneDeviceSdk() {
  try {
    return deviceSdkPath().length > 0;
  } catch {
    return false;
  }
}

function buildIOSHost(scratchRoot) {
  const buildScratch = path.join(scratchRoot, "spm-build");
  const moduleCache = path.join(scratchRoot, "module-cache");
  const output = execFileSync(
    "swift",
    [
      "build",
      "--disable-sandbox",
      "--cache-path",
      path.join(scratchRoot, "swift-cache"),
      "--config-path",
      path.join(scratchRoot, "swift-config"),
      "--security-path",
      path.join(scratchRoot, "swift-security"),
      "--scratch-path",
      buildScratch,
      "--triple",
      "arm64-apple-ios17.0-simulator",
      "--sdk",
      simulatorSdkPath(),
      "-Xcc",
      `-fmodules-cache-path=${moduleCache}`,
      "-Xswiftc",
      "-module-cache-path",
      "-Xswiftc",
      moduleCache,
      "-Xswiftc",
      "-D",
      "-Xswiftc",
      "DEBUG",
    ],
    {
      cwd: iosDir,
      encoding: "utf8",
      env: {
        ...process.env,
        CLANG_MODULE_CACHE_PATH: moduleCache,
        SWIFT_MODULE_CACHE_PATH: moduleCache,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const binaryPath = path.join(buildScratch, "arm64-apple-ios-simulator", "debug", "TerraneHostIOS");
  return { buildScratch, binaryPath, output };
}

function buildIOSDeviceHost(scratchRoot, forgeFfiStaticLib) {
  const buildScratch = path.join(scratchRoot, "spm-device-build");
  const moduleCache = path.join(scratchRoot, "device-module-cache");
  const output = execFileSync(
    "swift",
    [
      "build",
      "--disable-sandbox",
      "--cache-path",
      path.join(scratchRoot, "swift-device-cache"),
      "--config-path",
      path.join(scratchRoot, "swift-device-config"),
      "--security-path",
      path.join(scratchRoot, "swift-device-security"),
      "--scratch-path",
      buildScratch,
      "--triple",
      "arm64-apple-ios17.0",
      "--sdk",
      deviceSdkPath(),
      "-Xcc",
      `-fmodules-cache-path=${moduleCache}`,
      "-Xswiftc",
      "-module-cache-path",
      "-Xswiftc",
      moduleCache,
      "-Xswiftc",
      "-D",
      "-Xswiftc",
      "DEBUG",
    ],
    {
      cwd: iosDir,
      encoding: "utf8",
      env: {
        ...process.env,
        CLANG_MODULE_CACHE_PATH: moduleCache,
        SWIFT_MODULE_CACHE_PATH: moduleCache,
        TERRANE_IOS_FORGE_FFI_STATICLIB: forgeFfiStaticLib,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const binaryPath = path.join(buildScratch, "arm64-apple-ios", "debug", "TerraneHostIOS");
  return { buildScratch, binaryPath, output };
}

function buildIOSForgeFfi() {
  execFileSync(
    "cargo",
    [
      "build",
      "-p",
      "forge-ffi",
      "--locked",
      "--target",
      "aarch64-apple-ios-sim",
    ],
    {
      cwd: path.join(repoRoot, "forge"),
      stdio: "ignore",
    },
  );
  const dylibPath = path.join(repoRoot, "forge", "target", "aarch64-apple-ios-sim", "debug", "libforge_ffi.dylib");
  assert.equal(fs.existsSync(dylibPath), true);
  const symbols = execFileSync("nm", ["-gU", dylibPath], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  assert.match(symbols, /_forge_core_open_in_memory/);
  assert.match(symbols, /_forge_core_handle_command/);
  assert.match(symbols, /_forge_string_free/);
  return dylibPath;
}

function buildIOSForgeFfiStatic() {
  execFileSync(
    "cargo",
    [
      "build",
      "-p",
      "forge-ffi",
      "--locked",
      "--target",
      "aarch64-apple-ios",
    ],
    {
      cwd: path.join(repoRoot, "forge"),
      stdio: "ignore",
    },
  );
  const libPath = path.join(repoRoot, "forge", "target", "aarch64-apple-ios", "debug", "libforge_ffi.a");
  assert.equal(fs.existsSync(libPath), true);
  return libPath;
}

function createSimulatorAppBundle(scratchRoot, binaryPath, forgeFfiDylibPath = null) {
  const appBundle = path.join(scratchRoot, "TerraneHostIOS.app");
  fs.mkdirSync(appBundle, { recursive: true });
  fs.copyFileSync(binaryPath, path.join(appBundle, "TerraneHostIOS"));
  fs.chmodSync(path.join(appBundle, "TerraneHostIOS"), 0o755);
  if (forgeFfiDylibPath) {
    const bundledCorePath = path.join(appBundle, "libforge_ffi.dylib");
    fs.copyFileSync(forgeFfiDylibPath, bundledCorePath);
    fs.chmodSync(bundledCorePath, 0o755);
    execFileSync("codesign", ["--force", "--sign", "-", bundledCorePath], { stdio: "ignore" });
  }

  fs.cpSync(path.join(repoRoot, "runtime-web"), path.join(appBundle, "runtime"), { recursive: true });
  fs.mkdirSync(path.join(appBundle, "webapps"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "webapps", "examples"), path.join(appBundle, "webapps", "examples"), { recursive: true });
  fs.mkdirSync(path.join(appBundle, "db"), { recursive: true });
  fs.cpSync(path.join(repoRoot, "db", "sqlite"), path.join(appBundle, "db", "sqlite"), { recursive: true });

  fs.writeFileSync(
    path.join(appBundle, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>TerraneHostIOS</string>
  <key>CFBundleIdentifier</key><string>${bundleId}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>TerraneHostIOS</string>
  <key>CFBundleDisplayName</key><string>TerraneHostIOS</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>17.0</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UIApplicationSupportsIndirectInputEvents</key><true/>
</dict>
</plist>
`,
  );

  execFileSync("codesign", ["--force", "--sign", "-", appBundle], { stdio: "ignore" });
  execFileSync("codesign", ["--verify", appBundle], { stdio: "ignore" });
  return appBundle;
}

function availableIOSDevices() {
  const listing = JSON.parse(execFileSync("xcrun", ["simctl", "list", "devices", "available", "--json"], { encoding: "utf8" }));
  return Object.entries(listing.devices ?? {})
    .filter(([runtime]) => runtime.includes("iOS"))
    .flatMap(([, devices]) => devices)
    .filter((device) => device.isAvailable && device.name.includes("iPhone"));
}

function selectIOSDevice() {
  if (process.env.TERRANE_IOS_SMOKE_DEVICE) {
    const devices = availableIOSDevices();
    return devices.find((device) => device.udid === process.env.TERRANE_IOS_SMOKE_DEVICE) ??
      { udid: process.env.TERRANE_IOS_SMOKE_DEVICE, state: "Unknown" };
  }
  const devices = availableIOSDevices();
  return devices.find((device) => device.state === "Booted") ??
    devices.find((device) => device.name === "iPhone 17") ??
    devices[0];
}

function currentDeviceState(udid) {
  return availableIOSDevices().find((device) => device.udid === udid)?.state ?? "Unknown";
}

function waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, expectedMarker, timeoutMs }) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const markerFile = fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8") : "";
    const stdout = fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "";
    const stderr = fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "";
    if (`${markerFile}\n${stdout}\n${stderr}`.includes(expectedMarker)) {
      return { markerFile, stdout, stderr };
    }
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
  }
  return {
    markerFile: fs.existsSync(markerPath) ? fs.readFileSync(markerPath, "utf8") : "",
    stdout: fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "",
    stderr: fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "",
  };
}

function launchAndWaitForMarker({ device, scratchRoot, markerPath, expectedMarker, launchArgs }) {
  fs.rmSync(markerPath, { force: true });
  const logStem = expectedMarker.toLowerCase().replaceAll("_", "-");
  const stdoutPath = path.join(scratchRoot, `${logStem}.stdout.log`);
  const stderrPath = path.join(scratchRoot, `${logStem}.stderr.log`);
  fs.rmSync(stdoutPath, { force: true });
  fs.rmSync(stderrPath, { force: true });

  const launcher = spawn(
    "xcrun",
    [
      "simctl",
      "launch",
      "--terminate-running-process",
      `--stdout=${stdoutPath}`,
      `--stderr=${stderrPath}`,
      device.udid,
      bundleId,
      ...launchArgs,
    ],
    { detached: true, stdio: "ignore" },
  );
  launcher.unref();

  const logs = waitForSmokeMarker({ markerPath, stdoutPath, stderrPath, expectedMarker, timeoutMs: 30_000 });
  if (launcher.pid) {
    try {
      process.kill(-launcher.pid, "SIGTERM");
    } catch {
      // The simctl launch process usually exits on its own after the app exits.
    }
  }
  if (!`${logs.markerFile}\n${logs.stdout}\n${logs.stderr}`.includes(expectedMarker)) {
    const screenshotPath = path.join(scratchRoot, `${logStem}.png`);
    execFileSync("xcrun", ["simctl", "io", device.udid, "screenshot", screenshotPath], { stdio: "ignore" });
    assert.fail(`iOS smoke marker ${expectedMarker} was not emitted; marker: ${markerPath}; screenshot: ${screenshotPath}\nmarker file:\n${logs.markerFile}\nstdout:\n${logs.stdout}\nstderr:\n${logs.stderr}`);
  }
}

function launchInSimulator({ scratchRoot, appBundle }) {
  const device = selectIOSDevice();
  assert.ok(device?.udid, "an available iOS simulator device should exist");

  const wasBooted = device.state === "Booted";
  if (!wasBooted) {
    execFileSync("xcrun", ["simctl", "boot", device.udid], { stdio: "ignore", timeout: 30_000 });
    try {
      execFileSync("xcrun", ["simctl", "bootstatus", device.udid, "-b"], { stdio: "ignore", timeout: 60_000 });
    } catch (error) {
      if (currentDeviceState(device.udid) !== "Booted") {
        throw error;
      }
    }
  }

  try {
    execFileSync("xcrun", ["simctl", "install", device.udid, appBundle], { stdio: "ignore", timeout: 60_000 });
    const dataContainer = execFileSync("xcrun", ["simctl", "get_app_container", device.udid, bundleId, "data"], { encoding: "utf8", timeout: 30_000 }).trim();
    const markerPath = path.join(dataContainer, "tmp", smokeMarkerFile);

    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeLoadedMarker,
      launchArgs: ["--terrane-smoke-runtime-load", "--terrane-smoke-exit-on-runtime-load"],
    });

    const storageKey = `notes-lite:ios-smoke-${process.pid}-${Date.now()}`;
    const storageValue = `ios-smoke-${process.pid}-${Date.now()}`;
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeStorageSetMarker,
      launchArgs: [
        "--terrane-smoke-storage-set",
        "--terrane-smoke-storage-key",
        storageKey,
        "--terrane-smoke-storage-value",
        storageValue,
        "--terrane-smoke-exit-on-runtime-load",
      ],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeStorageGetMarker,
      launchArgs: [
        "--terrane-smoke-storage-get",
        "--terrane-smoke-storage-key",
        storageKey,
        "--terrane-smoke-storage-value",
        storageValue,
        "--terrane-smoke-exit-on-runtime-load",
      ],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeStorageResetMarker,
      launchArgs: ["--terrane-smoke-storage-reset", "--terrane-smoke-exit-on-runtime-load"],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeCoreStepMarker,
      launchArgs: ["--terrane-smoke-core-step", "--terrane-smoke-exit-on-runtime-load"],
    });
    launchAndWaitForMarker({
      device,
      scratchRoot,
      markerPath,
      expectedMarker: smokeAllExamplesMarker,
      launchArgs: ["--terrane-smoke-all-examples", "--terrane-smoke-exit-on-runtime-load"],
    });
  } finally {
    if (!wasBooted) {
      execFileSync("xcrun", ["simctl", "shutdown", device.udid], { stdio: "ignore", timeout: 30_000 });
    }
  }
}

test("iOS debug dev control health endpoint is source-wired and token-gated", () => {
  const control = fs.readFileSync(path.join(iosDir, "Sources", "TerraneHostIOS", "IOSDevControlPlane.swift"), "utf8");
  const host = fs.readFileSync(path.join(iosDir, "Sources", "TerraneHostIOS", "WebHostView.swift"), "utf8");
  const bridge = fs.readFileSync(path.join(iosDir, "Sources", "TerraneHostIOS", "WebBridge.swift"), "utf8");
  const network = fs.readFileSync(path.join(iosDir, "Sources", "TerraneHostIOS", "PlatformNetwork.swift"), "utf8");
  const dialogs = fs.readFileSync(path.join(iosDir, "Sources", "TerraneHostIOS", "PlatformDialogs.swift"), "utf8");

  for (const snippet of [
    "#if DEBUG && targetEnvironment(simulator)",
    "import CryptoKit",
    "import Network",
    "import Security",
    "import SQLite3",
    "SecRandomCopyBytes",
    "PLATFORM_CONTROL_TOKEN_FILE",
    "TERRANE_IOS_DEV_CONTROL",
    "--terrane-dev-control",
    "--control-plane-port",
    "/control/sessions",
    "/control/command",
    "/capabilities",
    "/command",
    ".applicationSupportDirectory",
    "terrane",
    "control.token",
    "x-platform-control-token",
    "parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(IPv4Address(\"127.0.0.1\")!), port: listenPort)",
    "TERRANE_IOS_CONTROL_READY port=",
    "Control token is required",
    "control_auth_required",
    "request.method == \"GET\" && request.normalizedPath == \"/health\"",
    "request.method == \"POST\" && isSessionCreatePath(request.normalizedPath)",
    "request.method == \"POST\" && isCommandPath(request.normalizedPath)",
    "\"target\": \"ios-simulator\"",
    "\"loopback\": true",
    ".posixPermissions: 0o600",
    "INSERT OR REPLACE INTO control_sessions",
    "INSERT INTO control_commands",
    "UPDATE control_sessions SET status = 'ended'",
    "INSERT OR REPLACE INTO runtime_sessions",
    "UPDATE runtime_sessions SET status = 'ended'",
    "token_hash",
    "control.sessions.create",
    "control.sessions.snapshot",
    "control.sessions.events",
    "control.sessions.capabilities",
    "control.sessions.end",
    "\"platform.list_targets\"",
    "\"platform.list_webapps\"",
    "\"runtime.capabilities\"",
    "\"runtime.call_bridge\"",
    "\"runtime.core_step\"",
    "\"runtime.fault_inject\"",
    "\"runtime.network_mock_set\"",
    "\"runtime.network_mock_reset\"",
    "\"runtime.dialog_mock_set\"",
    "\"runtime.storage_get\"",
    "\"runtime.storage_set\"",
    "\"runtime.assert_storage\"",
    "\"runtime.storage_reset\"",
    "\"platform.reset_webapp\"",
    "\"runtime.accessibility_snapshot\"",
    "\"runtime.run_accessibility_audit\"",
    "\"runtime.assert_accessibility\"",
    "\"runtime.run_smoke_tests\"",
    "\"runtime.run_microtest\"",
    "\"platform.run_platform_smoke\"",
    "\"runtime.resource_usage\"",
    "\"runtime.event_log\"",
    "\"runtime.console_logs\"",
    "\"runtime.bridge_calls\"",
    "\"runtime.clear_logs\"",
    "\"runtime.notification_capture\"",
    "\"runtime.assert_bridge_call\"",
    "\"runtime.assert_no_console_errors\"",
    "\"runtime.core_snapshot\"",
    "\"runtime.replay_events\"",
    "\"runtime.assert_core_action\"",
    "\"platform.create_snapshot\"",
    "\"platform.restore_snapshot\"",
    "\"runtime.compare_snapshot\"",
    "\"db.snapshot\"",
    "\"db.query_app_storage\"",
    "\"db.query_app_versions\"",
    "\"db.query_bridge_calls\"",
    "\"db.query_core_events\"",
    "\"db.query_test_runs\"",
    "\"db.export_backup\"",
    "\"db.import_backup\"",
    "\"db.export_debug_bundle\"",
    "dbToolName(forPath",
    "dispatchDbTool",
    "SafeDbTable",
    "safeDbTableByTool",
    "dbSnapshotTables",
    "safeTableRows",
    "/db/snapshot",
    "/db/app-storage",
    "/db/app-versions",
    "/db/bridge-calls",
    "/db/core-events",
    "/db/test-runs",
    "/db/export-backup",
    "/db/import-backup",
    "/db/export-debug-bundle",
    "/control/db/",
    "INSERT OR REPLACE INTO backup_exports",
    "dbExportBackup",
    "dbImportBackup",
    "dbExportDocument",
    "db.import_backup requires backup",
    "Backup import requires type backup, debug-bundle, or test-fixture",
    "Backup import document is missing required arrays",
    "backupDbAppVersions",
    "backupDbAppFiles",
    "backupDbAppPermissions",
    "backupDbAppInstallations",
    "backupDbAppMigrations",
    "backupDbAppInstallReports",
    "requiredBackupArray",
    "requiredBackupString",
    "backupJsonText",
    "executePreparedStatement",
    "importAppVersion",
    "importAppFile",
    "importAppPermission",
    "importAppInstallation",
    "importBackupStorageRow",
    "importAppMigration",
    "importAppInstallReport",
    "INSERT OR REPLACE INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at)",
    "INSERT OR REPLACE INTO apps",
    "INSERT OR REPLACE INTO app_versions",
    "INSERT OR REPLACE INTO app_files",
    "INSERT OR REPLACE INTO app_permissions",
    "INSERT OR REPLACE INTO app_installations",
    "INSERT OR REPLACE INTO app_storage",
    "INSERT OR REPLACE INTO app_migrations",
    "INSERT OR REPLACE INTO app_install_reports",
    "\"appVersions\"",
    "\"appFiles\"",
    "\"appPermissions\"",
    "\"appInstallations\"",
    "\"appStorage\"",
    "\"appMigrations\"",
    "\"appInstallReports\"",
    "\"debug-bundle\"",
    "\"sha256:\"",
    "source_platform",
    "LIMIT ?",
    "appFilterColumn",
    "requiresAppId",
    "\"platform.health\"",
    "\"accepted\"",
    "\"rejected\"",
    "BundledAppCatalog",
    "control_call_bridge",
    "control_core_step",
    "runtimeFaultInject",
    "runtimeNetworkMockSet",
    "runtimeNetworkMockReset",
    "runtimeDialogMockSet",
    "runtime.fault_inject requires a bridge method",
    "runtime.fault_inject appId is not a valid generated app id",
    "runtime.network_mock_set requires urlPattern or match.url and response",
    "runtime.dialog_mock_set requires dialogType or method",
    "Runtime effect mock appId is not a valid generated app id",
    "INSERT INTO fault_injections (fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at)",
    "INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at)",
    "DELETE FROM network_mocks WHERE session_id = ? AND app_id = ?",
    "DELETE FROM network_mocks WHERE session_id = ?",
    "DELETE FROM network_mocks WHERE app_id = ?",
    "DELETE FROM network_mocks",
    "INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at)",
    "fault_ios_",
    "netmock_ios_",
    "dialogmock_ios_",
    "fault_injected",
    "Injected bridge fault",
    "control_storage_get",
    "control_storage_set",
    "control_storage_assert_get",
    "method: \"storage.get\"",
    "method: \"storage.set\"",
    "runtime.storage_set requires appId, key, and value",
    "runtime.assert_storage requires appId, key, and value",
    "args[\"confirm\"] as? Bool == true",
    "requires confirm: true",
    "PlatformStorage().resetAppStorage",
    "snapshotId",
    "clearedStorageKeys",
    "clearedLogs",
    "bridgeCallsCleared",
    "coreActionsCleared",
    "coreEventsCleared",
    "DELETE FROM bridge_calls WHERE app_id = ?",
    "DELETE FROM core_events WHERE app_id = ?",
    "DELETE FROM core_actions WHERE app_id = ?",
    "SELECT COUNT(*) FROM app_storage WHERE app_id = ?",
    "networkRequestsLastMinute",
    "logLinesLastMinute",
    "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at",
    "runtime.assert_bridge_call requires appId and method",
    "Expected bridge call was not recorded",
    "Console error logs were found",
    "console_errors_found",
    "notification.toast",
    "app.log",
    "coreSnapshot",
    "replayEvents",
    "assertCoreAction",
    "runtime.core_snapshot requires appId",
    "runtime.replay_events requires appId",
    "runtime.replay_events events must be an array",
    "runtime.assert_core_action requires appId",
    "runtime.assert_core_action type must be a string",
    "runtime.assert_core_action match must be an object",
    "core_action.not_found",
    "Expected core action was not found",
    "control_replay_",
    "ForgeCoreBridge()",
    "safeDbCoreEvents",
    "safeDbCoreActions",
    "parsedCoreRows",
    "createSnapshot",
    "restoreSnapshot",
    "compareSnapshot",
    "platform.create_snapshot requires appId",
    "platform.restore_snapshot requires confirm: true",
    "platform.restore_snapshot requires snapshotId",
    "runtime.compare_snapshot requires left/right snapshots or snapshot ids",
    "snapshot_not_found",
    "snapshotTypes",
    "readSnapshot",
    "comparableSnapshotValue",
    "snapshot.removeValue(forKey: \"appStorage\")",
    "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key",
    "SELECT snapshot_id, snapshot_json, content_hash, created_at FROM runtime_snapshots WHERE snapshot_id = ?",
    "INSERT INTO runtime_snapshots",
    "DELETE FROM app_storage WHERE app_id = ?",
    "INSERT OR REPLACE INTO app_storage",
    "restoredStorageKeys",
    "leftHash",
    "rightHash",
    "contentHash",
    "accessibilitySnapshot",
    "accessibilityAudit",
    "assertAccessibility",
    "runSmokeTests",
    "runMicrotest",
    "runPlatformSmoke",
    "evaluateSmokeTests",
    "evaluateMicrotestSpec",
    "staticStepResult",
    "recordTestRun",
    "INSERT INTO micro_tests",
    "INSERT INTO test_runs",
    "ios-static-smoke",
    "ios-static-microtest",
    "ios-static-platform-smoke",
    "runtime.run_smoke_tests requires appId",
    "runtime.run_microtest requires spec or microtestPath",
    "platform.run_platform_smoke requires spec or smokePath",
    "platform.run_platform_smoke requires an apps array",
    "bridge.call_missing",
    "selector.not_found",
    "text.not_found",
    "htmlForStaticApp",
    "RuntimeResourceLocator.fileURL(forRuntimeURL",
    "accessibilitySnapshotFromHtml",
    "accessibilityAuditFromHtml",
    "document_title",
    "main_landmark",
    "screen_title",
    "no_unlabeled_controls",
    "Every interactive control must have an accessible name.",
    "accessibility_failed",
    "Generated app HTML was not found",
    "controlRecords",
    "accessibleName",
    "parseHtmlAttrs",
    "firstHtmlMatch",
    "Storage value did not match expected value",
  ]) {
    assert.equal(control.includes(snippet), true, `iOS dev control source should contain ${snippet}`);
  }
  assert.equal(control.includes("db.query_sql"), false);
  assert.equal(control.includes("unsafe_eval"), false);
  assert.equal(control.includes("sqlite3_exec"), false);
  assert.equal(control.includes("SELECT *"), false);

  for (const snippet of [
    "handleControlBridgeCall",
    "AppSandboxContext(controlAppId: appId, mountToken: \"ios-dev-control\")",
    "init(controlAppId appId: String, mountToken: String?)",
    "struct BridgeResponse: @unchecked Sendable",
    "faultInjectionFailure",
    "SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections",
    "UPDATE fault_injections SET enabled = 0 WHERE fault_id = ?",
  ]) {
    assert.equal(bridge.includes(snippet), true, `iOS bridge should expose dev control bridge routing with ${snippet}`);
  }

  for (const snippet of [
    "SELECT response_json, url_pattern FROM network_mocks",
    "mockedNetworkResponse",
    "urlMatches",
    "delayMs",
  ]) {
    assert.equal(network.includes(snippet), true, `iOS network mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "SELECT response_json FROM dialog_mocks",
    "storedDialogMock",
  ]) {
    assert.equal(dialogs.includes(snippet), true, `iOS dialog mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "#if targetEnvironment(simulator)",
    "let devControlPlane: IOSDevControlPlane?",
    "IOSDevControlPlane.enabledFromProcess(bridge: bridge)",
    "devControlPlane?.start()",
    "devControlPlane?.stop()",
  ]) {
    assert.equal(host.includes(snippet), true, `iOS host should wire dev control with ${snippet}`);
  }
});

test("iOS Package.swift exposes a device-safe Forge FFI static-link hook", () => {
  const manifest = fs.readFileSync(path.join(iosDir, "Package.swift"), "utf8");
  for (const snippet of [
    "TERRANE_IOS_FORGE_FFI_STATICLIB",
    "forgeFfiLinkerSettings",
    ".linkedLibrary(\"sqlite3\")",
    ".unsafeFlags",
    "\"-force_load\"",
    "linkerSettings: forgeFfiLinkerSettings",
  ]) {
    assert.equal(manifest.includes(snippet), true, `iOS package manifest should contain ${snippet}`);
  }
});

test(
  "iOS device build can static-link Forge FFI",
  {
    skip: process.env.TERRANE_IOS_DEVICE_STATIC_LINK !== "1"
      ? "set TERRANE_IOS_DEVICE_STATIC_LINK=1 to run the iPhoneOS static-link smoke"
      : process.platform !== "darwin"
        ? "iOS device static-link smoke only runs on Darwin hosts"
        : !commandWorks("swift", ["--version"])
          ? "swift is not available"
          : !commandWorks("cargo", ["--version"])
            ? "cargo is not available"
            : !hasIPhoneDeviceSdk()
              ? "iPhoneOS SDK is not available"
              : false,
    timeout: 180_000,
  },
  () => {
    const scratchRoot = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-ios-device-link-"));
    try {
      const forgeFfiStaticPath = buildIOSForgeFfiStatic();
      const build = buildIOSDeviceHost(scratchRoot, forgeFfiStaticPath);
      assert.match(build.output, /Build complete!/);
      assert.equal(fs.existsSync(build.binaryPath), true);

      const fileOutput = execFileSync("file", [build.binaryPath], { encoding: "utf8" });
      assert.match(fileOutput, /Mach-O 64-bit executable arm64/);
      const loadCommands = execFileSync("otool", ["-l", build.binaryPath], { encoding: "utf8" });
      assert.match(loadCommands, /platform 2/);
      assert.match(loadCommands, /minos 17\.0/);
      const linkedLibraries = execFileSync("otool", ["-L", build.binaryPath], { encoding: "utf8" });
      assert.match(linkedLibraries, /UIKit\.framework\/UIKit/);
      assert.match(linkedLibraries, /WebKit\.framework\/WebKit/);
      assert.doesNotMatch(linkedLibraries, /libforge_ffi\.dylib/);
    } finally {
      fs.rmSync(scratchRoot, { recursive: true, force: true });
    }
  },
);

test(
  "iOS native scaffold builds a simulator app bundle with runtime resources",
  {
    skip: process.platform !== "darwin"
      ? "iOS simulator build smoke only runs on Darwin hosts"
      : !commandWorks("swift", ["--version"])
        ? "swift is not available"
        : process.env.TERRANE_IOS_SMOKE_LAUNCH === "1" && !commandWorks("xcrun", ["simctl", "help"])
          ? "simctl is not available"
          : process.env.TERRANE_IOS_SMOKE_LAUNCH === "1" && !commandWorks("cargo", ["--version"])
            ? "cargo is not available"
            : !hasIPhoneSimulatorSdk()
              ? "iPhone simulator SDK is not available"
              : false,
    timeout: process.env.TERRANE_IOS_SMOKE_LAUNCH === "1" ? 180_000 : 120_000,
  },
  () => {
    const scratchRoot = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-ios-smoke-"));
    try {
      const build = buildIOSHost(scratchRoot);
      assert.match(build.output, /Build complete!/);
      assert.equal(fs.existsSync(build.binaryPath), true);

      const fileOutput = execFileSync("file", [build.binaryPath], { encoding: "utf8" });
      assert.match(fileOutput, /Mach-O 64-bit executable arm64/);
      const loadCommands = execFileSync("otool", ["-l", build.binaryPath], { encoding: "utf8" });
      assert.match(loadCommands, /platform 7/);
      assert.match(loadCommands, /minos 17\.0/);
      const linkedLibraries = execFileSync("otool", ["-L", build.binaryPath], { encoding: "utf8" });
      assert.match(linkedLibraries, /UIKit\.framework\/UIKit/);
      assert.match(linkedLibraries, /WebKit\.framework\/WebKit/);
      assert.match(linkedLibraries, /libsqlite3\.dylib/);

      const forgeFfiDylibPath = process.env.TERRANE_IOS_SMOKE_LAUNCH === "1" ? buildIOSForgeFfi() : null;
      const appBundle = createSimulatorAppBundle(scratchRoot, build.binaryPath, forgeFfiDylibPath);
      assert.equal(fs.existsSync(path.join(appBundle, "runtime", "index.html")), true);
      for (const appId of exampleAppIds) {
        for (const fileName of ["manifest.json", "index.html", "styles.css", "app.js"]) {
          assert.equal(fs.existsSync(path.join(appBundle, "webapps", "examples", appId, fileName)), true, `${appId}/${fileName} should be bundled`);
        }
      }
      assert.equal(fs.existsSync(path.join(appBundle, "db", "sqlite", "001_initial.sql")), true);
      if (forgeFfiDylibPath) {
        assert.equal(fs.existsSync(path.join(appBundle, "libforge_ffi.dylib")), true);
      }

      if (process.env.TERRANE_IOS_SMOKE_LAUNCH === "1") {
        launchInSimulator({ scratchRoot, appBundle });
      }
    } finally {
      fs.rmSync(scratchRoot, { recursive: true, force: true });
    }
  },
);
