import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const androidDir = path.join(repoRoot, "native", "android");
const packageName = "com.nativeai.platform";
const activityName = `${packageName}/.MainActivity`;
const smokeLogTag = "NativeAIPlatformSmoke";

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function commandExists(command) {
  try {
    const executable = command === "emulator" ? emulatorCommand() : command;
    const args = command === "zig" ? ["version"] : command === "emulator" ? ["-version"] : ["--version"];
    execFileSync(executable, args, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasAndroidSdk() {
  return Boolean(androidSdkPath());
}

function androidSdkPath() {
  return [process.env.ANDROID_HOME, process.env.ANDROID_SDK_ROOT, path.join(process.env.HOME ?? "", "Library", "Android", "sdk")]
    .filter(Boolean)
    .find((candidate) => fs.existsSync(candidate));
}

function emulatorCommand() {
  const sdkPath = androidSdkPath();
  const modern = sdkPath ? path.join(sdkPath, "emulator", "emulator") : null;
  return modern && fs.existsSync(modern) ? modern : "emulator";
}

function findFiles(directory, predicate) {
  if (!fs.existsSync(directory)) return [];
  const found = [];
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      found.push(...findFiles(absolutePath, predicate));
      continue;
    }
    if (predicate(absolutePath)) {
      found.push(absolutePath);
    }
  }
  return found;
}

test("Android WebView bridge setup is hardened before runtime load", () => {
  const activity = read("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");

  for (const snippet of [
    "WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)",
    "Android WebMessageListener support is required for NativeAI runtime bridge",
    "WebViewCompat.addWebMessageListener",
    "setOf(\"https://appassets.androidplatform.net\")",
    "allowFileAccess = false",
    "allowFileAccessFromFileURLs = false",
    "allowUniversalAccessFromFileURLs = false",
    "allowContentAccess = false",
    "WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)",
    "safeBrowsingEnabled = true",
  ]) {
    assert.equal(activity.includes(snippet), true, `Android MainActivity should contain ${snippet}`);
  }

  assert.equal(activity.includes("addJavascriptInterface"), false);
});

test("Android debug dev control plane is loopback-bound, token-gated, audited, and bridge-routed", () => {
  const activity = read("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");
  const bridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");

  for (const snippet of [
    "AndroidDevControlPlane",
    "BuildConfig.DEBUG",
    "native_ai_control_port",
    "Android dev control plane is disabled in release builds",
    "devControlPlane?.stop()",
  ]) {
    assert.equal(activity.includes(snippet), true, `Android activity should contain ${snippet}`);
  }

  for (const snippet of [
    "BuildConfig.DEBUG",
    "ServerSocket(requestedPort, 50, InetAddress.getByName(\"127.0.0.1\"))",
    "control.token",
    "Context.MODE_PRIVATE",
    "SecureRandom().nextBytes(bytes)",
    "Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING",
    "X-Platform-Control-Token",
    "headers[\"x-platform-control-token\"]",
    "control_auth_required",
    "HTTP/1.1 $status $statusText",
    "\"/health\"",
    "\"/control/sessions\"",
    "\"/control/command\"",
    "control.sessions.create",
    "control.sessions.snapshot",
    "control.sessions.events",
    "control.sessions.capabilities",
    "control.sessions.end",
    "platform.health",
    "platform.list_targets",
    "platform.list_webapps",
    "runtime.capabilities",
    "runtime.call_bridge",
    "runtime.core_step",
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "safeTables",
    "safeFilterColumns",
    "database.readableDatabase.query(table",
    "control_sessions",
    "control_commands",
    "insertWithOnConflict(\"control_sessions\"",
    "database.writableDatabase.insert(\"control_commands\"",
    "NATIVE_AI_ANDROID_CONTROL_READY port=",
    "Unsupported Android dev control command",
  ]) {
    assert.equal(control.includes(snippet), true, `Android dev control source should contain ${snippet}`);
  }

  for (const forbidden of [
    "rawQuery(\"SELECT *",
    "db.query_sql",
    "execSQL(args",
    "addJavascriptInterface",
  ]) {
    assert.equal(control.includes(forbidden), false, `Android dev control source should not contain ${forbidden}`);
  }

  for (const snippet of [
    "handleControlBridgeCall",
    "mountToken\" to \"android-dev-control\"",
    "sourceOrigin = trustedRuntimeOrigin",
    "CountDownLatch",
    "Timed out waiting for native bridge response",
  ]) {
    assert.equal(bridge.includes(snippet), true, `Android bridge should expose control dispatch snippet ${snippet}`);
  }
});

test("Android debug dev control exposes target and webapp listing controls", () => {
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");

  for (const snippet of [
    "platform.list_targets",
    "platform.list_webapps",
    "platformListTargetsJson",
    "platformListWebappsJson",
    "appendBundledWebapp",
    "bundledManifest",
    "knownBundledAppIds",
    "\"android-emulator\"",
    "\"available\"",
    "includeUninstalled",
    "webapps/examples/$appId/manifest.json",
    "notes-lite",
    "task-workbench",
    "api-dashboard",
    "bundled",
    "installed",
    "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version",
    "LEFT JOIN app_versions v ON v.install_id = a.active_install_id",
  ]) {
    assert.equal(control.includes(snippet), true, `Android list control source should contain ${snippet}`);
  }
});

test("Android debug dev control exposes storage and basic observability commands", () => {
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");
  const bridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");

  for (const snippet of [
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.assert_storage",
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
    "runtime.bridge_calls",
    "runtime.clear_logs",
    "runtime.notification_capture",
    "runtime.assert_bridge_call",
    "runtime.assert_no_console_errors",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "storageGetParams",
    "storageSetParams",
    "runtimeAssertStorage",
    "runtimeResourceUsageJson",
    "runtimeEventLogJson",
    "runtimeConsoleLogsJson",
    "runtimeBridgeCallsJson",
    "runtimeClearLogsJson",
    "runtimeNotificationCaptureJson",
    "runtimeAssertBridgeCallJson",
    "runtimeAssertNoConsoleErrorsJson",
    "runtimeStorageResetJson",
    "optionalString",
    "deleteLogRows",
    "android_control_storage_get",
    "android_control_storage_set",
    "android_control_storage_assert_get",
    "storage.get",
    "storage.set",
    "runtime.storage_get requires appId and key",
    "runtime.storage_set requires appId, key, and value",
    "runtime.assert_storage requires appId, key, and value",
    "Storage value did not match expected value",
    "Storage reset command requires confirm: true",
    "confirmation_required",
    "insertOrThrow(\"runtime_snapshots\"",
    "runtime_snapshots",
    "snapshot_android_",
    "sha256:",
    "db.delete(\"app_storage\", \"app_id = ?\"",
    "db.delete(\"bridge_calls\", \"app_id = ?\"",
    "db.delete(\"core_events\", \"app_id = ?\"",
    "clearedStorageKeys",
    "storageRowsDeleted",
    "clearedBridgeCalls",
    "clearedCoreEvents",
    "clearedCoreActions",
    "SELECT COUNT(*) FROM app_storage WHERE app_id = ?",
    "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?",
    "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?",
    "SELECT COUNT(*) FROM core_events WHERE app_id = ?",
    "SELECT COUNT(*) FROM core_actions WHERE app_id = ?",
    "WHERE app_id = ? AND method = 'app.log'",
    "WHERE app_id = ? AND method = 'notification.toast'",
    "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at",
    "bridgeCallRows",
    "notificationRows",
    "Expected bridge call was not recorded",
    "Console error logs were found",
    "console_errors_found",
    "bridgeCallsCleared",
    "coreActionsCleared",
    "coreEventsCleared",
    "jsonValuesEqual",
    "parseJsonValue",
    "scalarLong",
    "consoleLogRows",
  ]) {
    assert.equal(control.includes(snippet), true, `Android storage/observability control source should contain ${snippet}`);
  }

  assert.equal(bridge.includes("recordBridgeCall(request, responseText, startedAtMs)"), true);
  assert.equal(bridge.includes("recordCoreStep(request, responseText)"), true);
});

test("Android debug dev control registers and consumes DB-backed bridge fault injections", () => {
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");
  const bridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");

  for (const snippet of [
    "runtime.fault_inject",
    "runtimeFaultInjectJson",
    "faultMethodForArgs",
    "faultDetailsForArgs",
    "fault_android_",
    "fault_injections",
    "runtime.fault_inject requires a bridge method",
    "runtime.fault_inject appId is not a valid generated app id",
    "Unknown bridge method:",
    "fault_injected",
    "Injected bridge fault",
    "put(\"once\", if (once) 1 else 0)",
    "put(\"enabled\", 1)",
    "\"storage.read\" -> \"storage.get\"",
    "\"storage.write\" -> \"storage.set\"",
    "\"network\", \"network.request\" -> \"network.request\"",
    "\"core\", \"core.step\" -> \"core.step\"",
    "knownBridgeMethods",
  ]) {
    assert.equal(control.includes(snippet), true, `Android dev control fault source should contain ${snippet}`);
  }

  for (const snippet of [
    "val faultResponse = faultInjectionFailure(request)",
    "faultInjectionFailure",
    "SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections",
    "WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)",
    "ORDER BY created_at LIMIT 1",
    "details.put(\"faultId\", faultId)",
    "details.put(\"appId\", request.context.appId)",
    "details.put(\"method\", request.method)",
    "disableFaultInjection",
    "ContentValues().apply { put(\"enabled\", 0) }",
    "BridgeResponse.failure(request.id, fault.code, fault.message, fault.details)",
    "private data class InjectedFault",
  ]) {
    assert.equal(bridge.includes(snippet), true, `Android bridge fault source should contain ${snippet}`);
  }

  assert.ok(
    bridge.indexOf("val faultResponse = faultInjectionFailure(request)") > bridge.indexOf("request.params.has(\"appId\")"),
    "fault injection should not mask appId-in-params security validation",
  );
  assert.ok(
    bridge.indexOf("val faultResponse = faultInjectionFailure(request)") < bridge.indexOf("val permission = permissionForBridgeMethod(request.method)"),
    "fault injection should run before permission and budget dispatch",
  );
});

test("Android debug dev control registers and consumes DB-backed network and dialog mocks", () => {
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");
  const bridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");
  const network = read("native/android/app/src/main/java/com/nativeai/platform/PlatformNetwork.kt");
  const dialogs = read("native/android/app/src/main/java/com/nativeai/platform/PlatformDialogs.kt");
  const migration = read("db/sqlite/003_codex_control.sql");

  for (const snippet of [
    "runtime.network_mock_set",
    "runtime.network_mock_reset",
    "runtime.dialog_mock_set",
    "runtimeNetworkMockSetJson",
    "runtimeNetworkMockResetJson",
    "runtimeDialogMockSetJson",
    "networkMockUrlPattern",
    "networkMockMethod",
    "dialogMockType",
    "dialogMockResponse",
    "Runtime effect mock appId is not a valid generated app id",
    "runtime.network_mock_set requires urlPattern or match.url and response",
    "runtime.dialog_mock_set requires dialogType or method",
    "Network mock could not be registered",
    "Dialog mock could not be registered",
    "netmock_android_",
    "dialogmock_android_",
    "database.writableDatabase.insert(\"network_mocks\"",
    "database.writableDatabase.insert(\"dialog_mocks\"",
    "database.writableDatabase.delete(\"network_mocks\"",
    "put(\"response_json\", jsonString(args.opt(\"response\")))",
    "put(\"response_json\", jsonString(dialogMockResponse(args)))",
    "uppercase(Locale.US)",
    "raw.removePrefix(\"dialog.\")",
  ]) {
    assert.equal(control.includes(snippet), true, `Android dev control mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "PlatformNetwork(database)",
  ]) {
    assert.equal(bridge.includes(snippet), true, `Android bridge mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "class PlatformNetwork(private val database: PlatformDatabase? = null)",
    "val mocked = mockedNetworkResponse(request, rule, urlText, method, effectiveTimeoutMs)",
    "mockedNetworkResponse",
    "findNetworkMock",
    "SELECT response_json, url_pattern FROM network_mocks",
    "WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)",
    "ORDER BY created_at DESC LIMIT 100",
    "urlMatches",
    "mockResponseBytes",
    "payloadWithoutDelay",
    "delayMs",
    "network.request timed out",
    "network.response exceeds manifest.networkPolicy maxResponseBytes",
  ]) {
    assert.equal(network.includes(snippet), true, `Android network mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "private val database = PlatformDatabase(activity)",
    "storedDialogMock(request, \"openFile\")",
    "storedDialogMock(request, \"saveFile\")",
    "SELECT response_json FROM dialog_mocks",
    "WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)",
    "ORDER BY created_at DESC LIMIT 1",
    "BridgeResponse.success(request.id, mock)",
  ]) {
    assert.equal(dialogs.includes(snippet), true, `Android dialog mock source should contain ${snippet}`);
  }

  assert.ok(
    network.indexOf("val mocked = mockedNetworkResponse(request, rule, urlText, method, effectiveTimeoutMs)") >
      network.indexOf("val effectiveTimeoutMs = effectiveTimeoutMs(rule, requestedTimeoutMs)"),
    "network mocks should run after manifest policy and timeout validation",
  );
  assert.ok(
    network.indexOf("val mocked = mockedNetworkResponse(request, rule, urlText, method, effectiveTimeoutMs)") <
      network.indexOf("return performRequestOffMainThread"),
    "network mocks should short-circuit before OkHttp dispatch",
  );
  assert.ok(
    dialogs.indexOf("storedDialogMock(request, \"openFile\")") < dialogs.indexOf("activity.runOnUiThread"),
    "dialog mocks should short-circuit before launching native pickers",
  );

  for (const snippet of [
    "CREATE TABLE IF NOT EXISTS network_mocks",
    "CREATE TABLE IF NOT EXISTS dialog_mocks",
    "idx_network_mocks_session_app",
    "idx_dialog_mocks_session_app",
  ]) {
    assert.equal(migration.includes(snippet), true, `SQLite mock migration should contain ${snippet}`);
  }
});

test("Android debug dev control exports DB-backed debug bundles safely", () => {
  const control = read("native/android/app/src/main/java/com/nativeai/platform/AndroidDevControlPlane.kt");
  const migrationRuntime = read("db/sqlite/002_runtime_debug.sql");
  const migrationBackup = read("db/sqlite/004_migrations_and_snapshots.sql");

  for (const snippet of [
    "db.export_debug_bundle",
    "dbExportDebugBundleJson",
    "debugbundle_android_",
    "\"type\", \"debug-bundle\"",
    "\"source_platform\", \"android\"",
    "runtimeVersion",
    "contentHash",
    "sha256:${sha256Hex(document.toString())}",
    "database.writableDatabase.insert(\"backup_exports\"",
    "Could not export debug bundle",
    "\"runtime_snapshots\" to tableRows(\"runtime_snapshots\")",
    "\"backup_exports\" to tableRows(\"backup_exports\")",
    "\"app_files\" to tableRows(\"app_files\")",
    "\"app_permissions\" to tableRows(\"app_permissions\")",
    "\"app_install_reports\" to tableRows(\"app_install_reports\")",
    "\"migration_runs\" to tableRows(\"migration_runs\")",
    "\"runtimeSnapshots\", tableRows(\"runtime_snapshots\")",
    "\"bridgeCalls\", tableRows(\"bridge_calls\")",
    "\"testRuns\", tableRows(\"test_runs\")",
    "private const val androidRuntimeVersion = \"0.1.0\"",
  ]) {
    assert.equal(control.includes(snippet), true, `Android debug bundle source should contain ${snippet}`);
  }

  for (const snippet of [
    "runtime_snapshots",
    "snapshot_json",
    "content_hash",
  ]) {
    assert.equal(migrationRuntime.includes(snippet), true, `Runtime snapshot migration should contain ${snippet}`);
  }

  for (const snippet of [
    "CREATE TABLE IF NOT EXISTS backup_exports",
    "export_json TEXT NOT NULL",
    "idx_backup_exports_created",
  ]) {
    assert.equal(migrationBackup.includes(snippet), true, `Backup export migration should contain ${snippet}`);
  }

  for (const forbidden of [
    "rawQuery(args",
    "execSQL(args",
    "db.query_sql",
  ]) {
    assert.equal(control.includes(forbidden), false, `Android debug bundle source should not expose ${forbidden}`);
  }
});

test(
  "Android native scaffold assembles debug APK with synced runtime assets and JNI libraries",
  {
    skip: !commandExists("gradle")
      ? "gradle is not available"
      : !commandExists("zig")
        ? "zig is not available"
        : !hasAndroidSdk()
          ? "Android SDK is not available"
          : false,
    timeout: 180_000,
  },
  () => {
    const output = execFileSync("gradle", ["--rerun-tasks", ":app:assembleDebug"], {
      cwd: androidDir,
      encoding: "utf8",
      env: process.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    assert.match(output, /BUILD SUCCESSFUL/);
    const apkPath = path.join(androidDir, "app", "build", "outputs", "apk", "debug", "app-debug.apk");
    assert.equal(fs.existsSync(apkPath), true);
    assert.equal(
      fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-assets", "runtime", "index.html")),
      true,
      "runtime-web assets should be synced under the Android /runtime asset path",
    );
    assert.equal(
      fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-assets", "webapps", "examples", "notes-lite", "manifest.json")),
      true,
      "generated example apps should be synced into Android assets",
    );
    assert.equal(
      fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-assets", "db", "sqlite", "001_initial.sql")),
      true,
      "checked-in SQLite migrations should be synced into Android assets",
    );

    for (const abi of ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"]) {
      assert.equal(
        fs.existsSync(path.join(androidDir, "app", "build", "generated", "native-ai-zig-core", "jniLibs", abi, "libzig_core.so")),
        true,
        `Zig core shared library should be generated for ${abi}`,
      );
      assert.equal(
        findFiles(path.join(androidDir, "app", "build", "intermediates", "cxx", "Debug"), (filePath) =>
          filePath.endsWith(path.join("obj", abi, "libzig_core_jni.so")),
        ).length > 0,
        true,
        `JNI bridge library should build for ${abi}`,
      );
      assert.equal(
        findFiles(path.join(androidDir, "app", "build", "intermediates", "merged_native_libs", "debug"), (filePath) =>
          filePath.endsWith(path.join("lib", abi, "libzig_core.so")),
        ).length > 0,
        true,
        `Zig core shared library should be packaged for ${abi}`,
      );
    }

    runOptionalEmulatorSmoke(apkPath);
  },
);

function runOptionalEmulatorSmoke(apkPath) {
  if (process.env.NATIVE_AI_ANDROID_SMOKE_LAUNCH !== "1") return;
  assert.equal(commandExists("adb"), true, "adb is required for Android emulator smoke");
  assert.equal(commandExists("emulator"), true, "emulator is required for Android emulator smoke");

  let emulatorProcess = null;
  let serial = process.env.NATIVE_AI_ANDROID_SMOKE_SERIAL || listAdbDevices()[0] || null;
  if (!serial) {
    const avd = process.env.NATIVE_AI_ANDROID_SMOKE_AVD || listAvds()[0];
    assert.ok(avd, "No Android AVD is available for emulator smoke");
    emulatorProcess = spawn(
      emulatorCommand(),
      [
        "-avd",
        avd,
        "-no-window",
        "-no-audio",
        "-no-boot-anim",
        "-gpu",
        "swiftshader_indirect",
        "-no-snapshot-load",
        "-no-snapshot-save",
      ],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
    serial = waitForAnyDevice();
  }

  try {
    waitForBoot(serial);
    adb(["-s", serial, "install", "-r", apkPath]);
    launchAndWaitForMarker(serial, "NATIVE_AI_ANDROID_SMOKE_RUNTIME_LOADED", [
      "--ez",
      "native_ai_smoke_runtime_load",
      "true",
    ]);

    const storageKey = `notes-lite:android-smoke-${process.pid}-${Date.now()}`;
    const storageValue = `android-smoke-${process.pid}-${Date.now()}`;
    launchAndWaitForMarker(serial, "NATIVE_AI_ANDROID_SMOKE_STORAGE_SET_OK", [
      "--es",
      "native_ai_smoke_storage_action",
      "set",
      "--es",
      "native_ai_smoke_storage_key",
      storageKey,
      "--es",
      "native_ai_smoke_storage_value",
      storageValue,
    ]);
    adb(["-s", serial, "shell", "am", "force-stop", packageName]);
    launchAndWaitForMarker(serial, "NATIVE_AI_ANDROID_SMOKE_STORAGE_GET_OK", [
      "--es",
      "native_ai_smoke_storage_action",
      "get",
      "--es",
      "native_ai_smoke_storage_key",
      storageKey,
      "--es",
      "native_ai_smoke_storage_value",
      storageValue,
    ]);

    adb(["-s", serial, "shell", "am", "force-stop", packageName]);
    launchAndWaitForMarker(serial, "NATIVE_AI_ANDROID_SMOKE_CORE_STEP_OK", [
      "--ez",
      "native_ai_smoke_core_step",
      "true",
    ]);
  } finally {
    try {
      if (serial) adb(["-s", serial, "shell", "am", "force-stop", packageName]);
    } catch {}
    if (emulatorProcess) {
      try {
        adb(["-s", serial, "emu", "kill"]);
      } catch {
        emulatorProcess.kill("SIGTERM");
      }
    }
  }
}

function launchAndWaitForMarker(serial, marker, extras) {
  adb(["-s", serial, "shell", "am", "force-stop", packageName]);
  adb(["-s", serial, "logcat", "-c"]);
  adb([
    "-s",
    serial,
    "shell",
    "am",
    "start",
    "-W",
    "-n",
    activityName,
    "--ez",
    "native_ai_smoke_exit_after",
    "true",
    ...extras,
  ]);
  waitForSmokeMarker(serial, marker);
}

function waitForSmokeMarker(serial, marker) {
  const deadline = Date.now() + 60_000;
  let latest = "";
  while (Date.now() < deadline) {
    latest = adb(["-s", serial, "logcat", "-d", "-v", "brief", "-s", smokeLogTag], { allowFailure: true });
    assert.equal(latest.includes("NATIVE_AI_ANDROID_SMOKE_FAILED"), false, `${latest}\n${coreLog(serial)}`);
    if (latest.includes(marker)) return;
    sleep(500);
  }
  assert.fail(`Timed out waiting for ${marker}\n${latest}`);
}

function coreLog(serial) {
  return adb(["-s", serial, "logcat", "-d", "-v", "brief", "-s", "NativeAIPlatformCore"], { allowFailure: true });
}

function waitForAnyDevice() {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const devices = listAdbDevices();
    if (devices.length > 0) return devices[0];
    sleep(1000);
  }
  assert.fail("Timed out waiting for Android emulator device");
}

function waitForBoot(serial) {
  adb(["-s", serial, "wait-for-device"]);
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const booted = adb(["-s", serial, "shell", "getprop", "sys.boot_completed"], { allowFailure: true }).trim();
    if (booted === "1") return;
    sleep(1000);
  }
  assert.fail(`Timed out waiting for Android device ${serial} to boot`);
}

function listAdbDevices() {
  const output = adb(["devices"], { allowFailure: true });
  return output
    .split(/\r?\n/)
    .slice(1)
    .map((line) => line.trim().split(/\s+/))
    .filter(([serial, state]) => serial && state === "device")
    .map(([serial]) => serial);
}

function listAvds() {
  return execFileSync(emulatorCommand(), ["-list-avds"], { encoding: "utf8" })
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function adb(args, { allowFailure = false } = {}) {
  try {
    return execFileSync("adb", args, { encoding: "utf8" });
  } catch (error) {
    if (allowFailure) return `${error.stdout ?? ""}\n${error.stderr ?? ""}`;
    throw error;
  }
}

function sleep(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}
