import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

test("Windows dev control health route is debug-only, loopback-bound, token-gated, and audited", () => {
  const main = read("native/windows/src/main.cpp");
  const control = read("native/windows/src/DevControlPlane.cpp");
  const header = read("native/windows/src/DevControlPlane.h");
  const cmake = read("native/windows/CMakeLists.txt");

  for (const snippet of [
    "DevControlPlaneConfig",
    "Start(DevControlPlaneConfig const& config",
    "void SetHost(WebViewHost* host)",
    "void Stop()",
    "uint16_t Port() const",
    "std::filesystem::path TokenPath() const",
  ]) {
    assert.equal(header.includes(snippet), true, `Windows dev control header should expose ${snippet}`);
  }

  for (const snippet of [
    "_DEBUG",
    "NATIVE_AI_WINDOWS_DEV_CONTROL",
    "--native-ai-dev-control",
    "--control-plane-port",
    "DevControlPlaneConfig config",
    "devControl->Start(config",
    "Windows dev control plane is disabled in release builds",
    "RecordProductionGuardAudit(L\"NATIVE_AI_WINDOWS_DEV_CONTROL\")",
    "devControl->SetHost(g_host.get())",
  ]) {
    assert.equal(main.includes(snippet), true, `Windows main should contain ${snippet}`);
  }

  for (const snippet of [
    "_DEBUG",
    "AF_INET",
    "INADDR_LOOPBACK",
    "bind(listenSocket",
    "listen(listenSocket",
    "accept(listenSocket",
    "SO_RCVTIMEO",
    "PLATFORM_CONTROL_TOKEN_FILE",
    "FOLDERID_LocalAppData",
    "NativeAIWebappPlatform",
    "control.token",
    "BCryptGenRandom",
    "Base64Url",
    "Sha256Hex",
    "CreateFileW",
    "X-Platform-Control-Token",
    "HeaderValue(request, \"X-Platform-Control-Token\") != WideToUtf8(token)",
    "control_auth_required",
    "SendJson(client, 401, body)",
    "Unauthorized",
    "path == \"/health\" && method != \"GET\"",
    "\"/health\"",
    "Content-Length",
    "IsSessionsCollectionPath",
    "SessionIdFromPath",
    "\"/control/sessions\"",
    "control.sessions.create",
    "control.sessions.snapshot",
    "control.sessions.events",
    "control.sessions.capabilities",
    "SessionCapabilitiesJson",
    "controlPlane",
    "control.sessions.command",
    "control.sessions.end",
    "runtime.call_bridge",
    "runtime.core_step",
    "runtime.screenshot",
    "runtime.query",
    "runtime.click",
    "runtime.type",
    "runtime.set_value",
    "runtime.press_key",
    "runtime.drag",
    "runtime.wait_for",
    "runtime.timer_advance",
    "runtime.assert_visible",
    "runtime.assert_text",
    "runtime.accessibility_snapshot",
    "runtime.run_accessibility_audit",
    "runtime.assert_accessibility",
    "runtime.run_smoke_tests",
    "runtime.run_microtest",
    "platform.run_platform_smoke",
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
    "runtime.bridge_calls",
    "runtime.clear_logs",
    "runtime.notification_capture",
    "runtime.assert_bridge_call",
    "runtime.assert_no_console_errors",
    "runtime.fault_inject",
    "runtime.core_snapshot",
    "runtime.replay_events",
    "runtime.assert_core_action",
    "runtime.network_mock_set",
    "runtime.network_mock_reset",
    "runtime.dialog_mock_set",
    "platform.list_targets",
    "platform.list_webapps",
    "PlatformListTargetsJson",
    "PlatformListWebappsJson",
    "RuntimeScreenshotJson",
    "RuntimeQueryJson",
    "RuntimeTargetCommandJson",
    "RuntimeWaitForJson",
    "RuntimeTimerAdvanceJson",
    "RuntimeAssertVisibleJson",
    "RuntimeAssertTextJson",
    "RuntimeAccessibilitySnapshotJson",
    "RuntimeAccessibilityAuditJson",
    "RuntimeAssertAccessibilityJson",
    "RuntimeRunSmokeTestsJson",
    "RuntimeRunMicrotestJson",
    "PlatformRunSmokeJson",
    "BundledWebappJson",
    "BundledManifest",
    "RuntimeResourceRoot",
    "resources",
    "webapps",
    "examples",
    "windows-native",
    "includeUninstalled",
    "platform.list_webapps args must be an object",
    "\\\"status\\\":\\\"bundled\\\"",
    "\\\"bundled\\\":true",
    "\\\"installed\\\":false",
    "notes-lite",
    "task-workbench",
    "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version",
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "runtime.assert_storage",
    "platform.create_snapshot",
    "platform.restore_snapshot",
    "runtime.compare_snapshot",
    "ResourceUsageJson",
    "EventLogJson",
    "ConsoleLogsJson",
    "RuntimeBridgeCallsJson",
    "ClearRuntimeLogsJson",
    "NotificationCaptureJson",
    "AssertBridgeCallJson",
    "AssertNoConsoleErrorsJson",
    "RuntimeFaultInjectJson",
    "RuntimeNetworkMockSetJson",
    "RuntimeNetworkMockResetJson",
    "RuntimeDialogMockSetJson",
    "RuntimeStorageGetJson",
    "RuntimeStorageSetJson",
    "RuntimeStorageResetJson",
    "RuntimeAssertStorageJson",
    "PlatformCreateSnapshotJson",
    "PlatformRestoreSnapshotJson",
    "RuntimeCompareSnapshotJson",
    "EvaluateMicrotestSpecJson",
    "RecordTestRun",
    "RecordControlStorageBridgeCall",
    "SnapshotStorageRowsJson",
    "db.export_backup",
    "DbExportBackupJson",
    "db.import_backup",
    "DbImportBackupJson",
    "db.export_debug_bundle",
    "DbExportDebugBundleJson",
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "SafeTableRowsJson",
    "DbSnapshotJson",
    "DbQueryRowsJson",
    "INSERT OR REPLACE INTO backup_exports",
    "Unsupported DB inspection command",
    "control_call_bridge",
    "control_core_step",
    "DevControlBridgeCall",
    "unsupported_tool",
    "platform.health",
    "Audit(L\"platform.health\"",
    "NATIVE_AI_WINDOWS_CONTROL_READY port=",
    "control_sessions",
    "control_commands",
    "UPDATE control_sessions SET status = 'ended'",
    "'windows'",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows dev control source should contain ${snippet}`);
  }

  for (const snippet of ["src/DevControlPlane.cpp", "ws2_32", "bcrypt"]) {
    assert.equal(cmake.includes(snippet), true, `Windows CMake should contain ${snippet}`);
  }
});

test("Windows dev control supports static runtime UI controls over bundled app HTML", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "runtime.screenshot",
    "runtime.query",
    "runtime.click",
    "runtime.type",
    "runtime.set_value",
    "runtime.press_key",
    "runtime.drag",
    "runtime.wait_for",
    "runtime.timer_advance",
    "runtime.assert_visible",
    "runtime.assert_text",
    "HtmlForBundledApp",
    "HtmlText",
    "RuntimeQueryMatches",
    "RuntimeScreenshotJson",
    "RuntimeQueryJson",
    "RuntimeTargetCommandJson",
    "RuntimeWaitForJson",
    "RuntimeTimerAdvanceJson",
    "RuntimeAssertVisibleJson",
    "RuntimeAssertTextJson",
    "TagForAttribute",
    "TestIdSelectorValue",
    "static-html-summary",
    "data-testid",
    "runtime.query requires appId",
    "runtime.screenshot requires appId",
    "runtime.assert_visible requires appId",
    "runtime.assert_text requires appId and text",
    "runtime.wait_for bridge_call requires appId and method",
    "runtime.wait_for requires appId for selector/text waits",
    "selector.not_found",
    "text.not_found",
    "wait_timeout",
    "Expected runtime condition did not appear",
    "sha256:",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows static UI control source should contain ${snippet}`);
  }
});

test("Windows dev control supports static accessibility controls over bundled app HTML", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "runtime.accessibility_snapshot",
    "runtime.run_accessibility_audit",
    "runtime.assert_accessibility",
    "RuntimeAccessibilitySnapshotJson",
    "RuntimeAccessibilityAuditJson",
    "RuntimeAssertAccessibilityJson",
    "AccessibilityControls",
    "AccessibilityControlsJson",
    "AccessibilityHeadingsJson",
    "AccessibilityFailsRule",
    "document_title",
    "main_landmark",
    "screen_title",
    "no_unlabeled_controls",
    "Accessibility appId is not a valid generated app id",
    "accessibility_failed",
    "Accessibility assertion failed",
    "ControlSessionAllowsApp",
    "HtmlForBundledApp(appId)",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows accessibility control source should contain ${snippet}`);
  }
});

test("Windows dev control supports static smoke, micro-test, and platform smoke runners", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "runtime.run_smoke_tests",
    "runtime.run_microtest",
    "platform.run_platform_smoke",
    "RuntimeRunSmokeTestsJson",
    "EvaluateSmokeTestsJson",
    "RuntimeRunMicrotestJson",
    "EvaluateMicrotestSpecJson",
    "PlatformRunSmokeJson",
    "RecordTestRun",
    "ControlSpecJson",
    "MicrotestTargetAppIdFromArgs",
    "PlatformSmokeAppIdsFromArgs",
    "INSERT INTO micro_tests",
    "ON CONFLICT(micro_test_id) DO UPDATE",
    "INSERT INTO test_runs",
    "smoke-tests.json",
    "windows-static-smoke",
    "windows-static-microtest",
    "windows-static-platform-smoke",
    "bridge.call_missing",
    "selector.not_found",
    "text.not_found",
    "runtime.run_smoke_tests requires appId",
    "runtime.run_microtest requires spec or microtestPath",
    "platform.run_platform_smoke requires spec or smokePath",
    "platform.run_platform_smoke requires an apps array",
    "platform.run_platform_smoke apps must be generated app ids",
    "Micro-test must target at least one app",
    "ControlSessionAllowsApp",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows static test-runner source should contain ${snippet}`);
  }
});

test("Windows dev control supports DB-backed network and dialog mocks", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");
  const bridge = read("native/windows/src/WebBridge.cpp");
  const network = read("native/windows/src/PlatformNetwork.cpp");

  for (const snippet of [
    "runtime.network_mock_set",
    "runtime.network_mock_reset",
    "runtime.dialog_mock_set",
    "runtime.network_mock_set requires urlPattern or match.url and response",
    "runtime.dialog_mock_set requires dialogType or method",
    "NetworkMockUrlPattern",
    "DialogMockType",
    "RuntimeNetworkMockSetJson",
    "RuntimeNetworkMockResetJson",
    "RuntimeDialogMockSetJson",
    "INSERT INTO network_mocks",
    "DELETE FROM network_mocks WHERE app_id = ?",
    "INSERT INTO dialog_mocks",
    "control_session_allows_app",
  ]) {
    const expected = snippet === "control_session_allows_app" ? "ControlSessionAllowsApp" : snippet;
    assert.equal(control.includes(expected), true, `Windows mock control source should contain ${expected}`);
  }

  for (const snippet of [
    "MockedDialogResponse",
    "SELECT response_json FROM dialog_mocks",
    "dialog.openFile",
    "dialog.saveFile",
    "network_.Request(request, DatabaseHandle())",
  ]) {
    assert.equal(bridge.includes(snippet), true, `Windows bridge mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "FindNetworkMock",
    "SELECT response_json, url_pattern FROM network_mocks",
    "UrlMatches",
    "MockedNetworkResponse",
    "delayMs",
    "MockPayloadWithoutDelay",
    "network.response exceeds manifest.networkPolicy maxResponseBytes",
  ]) {
    assert.equal(network.includes(snippet), true, `Windows network mock source should contain ${snippet}`);
  }
});

test("Windows dev control exposes DB-backed one-shot runtime fault injection", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");
  const bridge = read("native/windows/src/WebBridge.cpp");

  for (const snippet of [
    "runtime.fault_inject",
    "RuntimeFaultInjectJson",
    "runtime.fault_inject requires a bridge method",
    "Unknown bridge method",
    "fault_injected",
    "Injected bridge fault",
    "INSERT INTO fault_injections",
    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
    "faultId",
    "once",
    "runtime.fault_inject appId is not a valid generated app id",
    "ControlSessionAllowsApp",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows fault-injection control source should contain ${snippet}`);
  }

  for (const snippet of [
    "FaultInjectionFailure",
    "SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections",
    "WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)",
    "ORDER BY created_at LIMIT 1",
    "UPDATE fault_injections SET enabled = 0 WHERE fault_id = ?",
    "BridgeResponse::Failure(request.id, request.hasId",
    "RecordBridgeCall(request, faultResponse",
  ]) {
    assert.equal(bridge.includes(snippet), true, `Windows bridge fault source should contain ${snippet}`);
  }

  const appIdRejection = bridge.indexOf("Bridge params must not include appId");
  const faultCheck = bridge.indexOf("FaultInjectionFailure(request)");
  const permissionCheck = bridge.indexOf("permissionForBridgeMethod(request.method)");
  assert.equal(appIdRejection >= 0 && faultCheck > appIdRejection, true);
  assert.equal(permissionCheck > faultCheck, true);
});

test("Windows dev control supports direct storage get, set, and assertions", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "PlatformStorage storage(databasePath)",
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "runtime.assert_storage",
    "control_storage_get",
    "control_storage_set",
    "StorageBridgeRequest",
    "storage.get",
    "storage.set",
    "storage.read",
    "storage.write",
    "runtime.storage_get requires appId and key",
    "runtime.storage_set requires appId, key, and value",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "requires confirm: true",
    "confirmation_required",
    "RuntimeStorageResetJson",
    "INSERT INTO runtime_snapshots",
    "DELETE FROM app_storage WHERE app_id = ?",
    "clearedStorageKeys",
    "runtime.assert_storage requires appId, key, and value",
    "Expected storage key was not found",
    "Storage value did not match expected value",
    "CanonicalJsonValue",
    "RecordControlStorageBridgeCall",
    "INSERT INTO bridge_calls",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows storage control source should contain ${snippet}`);
  }
});

test("Windows dev control exposes explicit runtime snapshot create, restore, and compare controls", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "platform.create_snapshot",
    "platform.restore_snapshot",
    "runtime.compare_snapshot",
    "PlatformCreateSnapshotJson",
    "PlatformRestoreSnapshotJson",
    "RuntimeCompareSnapshotJson",
    "RuntimeSnapshotJsonById",
    "RuntimeSnapshotAppId",
    "ComparableSnapshotJson",
    "SnapshotCompareSkipMember",
    "SnapshotStorageSortKey",
    "SnapshotStorageRowsJson",
    "ValidSnapshotType",
    "OptionalArrayMember(snapshot, L\"appStorage\")",
    "keys[index] == L\"appStorage\"",
    "platform.create_snapshot requires appId",
    "platform.restore_snapshot requires confirm: true",
    "platform.restore_snapshot requires snapshotId",
    "snapshot_not_found",
    "runtime.compare_snapshot requires left/right snapshots or snapshot ids",
    "INSERT INTO runtime_snapshots",
    "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key",
    "SELECT snapshot_json, content_hash FROM runtime_snapshots WHERE snapshot_id = ?",
    "DELETE FROM app_storage WHERE app_id = ?",
    "Snapshot storage row app_id does not match snapshot appId",
    "Snapshot storage key is outside app storage prefix",
    "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
    "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
    "leftHash",
    "rightHash",
    "sha256:",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows snapshot control source should contain ${snippet}`);
  }

  assert.ok(
    control.indexOf("platform.create_snapshot") < control.indexOf("db.export_backup"),
    "snapshot controls should be first-class commands before DB export helpers",
  );
});

test("Windows dev control database inspection uses fixed allowlisted queries only", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "sqlite3_column_type",
    "SafeTableRowsJson(db, \"apps\"",
    "SafeTableRowsJson(db, \"app_storage\"",
    "SafeTableRowsJson(db, \"app_versions\"",
    "SafeTableRowsJson(db, \"bridge_calls\"",
    "SafeTableRowsJson(db, \"core_events\"",
    "SafeTableRowsJson(db, \"test_runs\"",
    "filterColumn",
    "filterValue",
    "LIMIT 100",
    "db.query_app_storage\" || tool == L\"db.query_app_versions",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows DB control source should contain ${snippet}`);
  }

  for (const forbidden of [
    "db.query_sql",
    "SELECT *",
    "OptionalStringMember(args.value(), L\"sql\")",
    "sqlite3_exec(db, WideToUtf8",
  ]) {
    assert.equal(control.includes(forbidden), false, `Windows DB control source should not contain ${forbidden}`);
  }
});

test("Windows dev control exports and imports portable backups through fixed DB tables", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "db.export_backup",
    "DbExportBackupJson",
    "DbExportDocumentJson",
    "db.import_backup",
    "DbImportBackupJson",
    "Backup import requires type backup, debug-bundle, or test-fixture",
    "Backup import document is missing required arrays",
    "BEGIN IMMEDIATE",
    "INSERT OR REPLACE INTO apps",
    "INSERT OR REPLACE INTO app_versions",
    "INSERT OR REPLACE INTO app_files",
    "INSERT OR REPLACE INTO app_permissions",
    "INSERT OR REPLACE INTO app_storage",
    "INSERT OR REPLACE INTO app_migrations",
    "INSERT OR REPLACE INTO app_install_reports",
    "INSERT INTO backup_exports",
    "VALUES (?, 'import'",
    "invalid_backup",
    "appVersions",
    "appStorage",
    "runtimeCapabilities",
    "contentHash",
    "sha256:",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows backup import/export source should contain ${snippet}`);
  }
});

test("Windows dev control exports debug bundles through backup_exports", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "db.export_debug_bundle",
    "DbExportDebugBundleJson",
    "debug-bundle",
    "runtimeCapabilities",
    "runtimeSessions",
    "bridgeCalls",
    "controlSessions",
    "controlCommands",
    "coreEvents",
    "coreActions",
    "runtimeSnapshots",
    "testRuns",
    "contentHash",
    "sha256:",
    "INSERT OR REPLACE INTO backup_exports",
    "source_platform",
    "runtime_version",
    "export_json",
    "content_hash",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows debug bundle source should contain ${snippet}`);
  }
});

test("Windows dev control exposes DB-backed resource and log inspection commands", () => {
  const control = read("native/windows/src/DevControlPlane.cpp");

  for (const snippet of [
    "OptionalArgsAppId",
    "runtime.resource_usage requires appId",
    "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?",
    "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?",
    "SELECT COUNT(*) FROM core_events WHERE app_id = ?",
    "networkRequestsLastMinute",
    "logLinesLastMinute",
    "runtime.event_log",
    "runtime.console_logs",
    "runtime.bridge_calls",
    "runtime.clear_logs",
    "runtime.notification_capture",
    "runtime.assert_bridge_call",
    "runtime.assert_no_console_errors",
    "BridgeCallRowsJson(db, appId)",
    "CoreEventRowsJson(db, appId)",
    "RuntimeBridgeCallsJson",
    "ClearRuntimeLogsJson",
    "NotificationCaptureJson",
    "AssertBridgeCallJson",
    "AssertNoConsoleErrorsJson",
    "RuntimeCoreSnapshotJson",
    "RuntimeReplayEventsJson",
    "RuntimeAssertCoreActionJson",
    "ZigCoreBridge replayCore",
    "control_replay_",
    "runtime.core_snapshot requires appId",
    "runtime.replay_events requires appId",
    "runtime.replay_events events must be an array",
    "runtime.assert_core_action requires appId",
    "core_action.not_found",
    "Expected core action was not found",
    "JsonMatchesSubset",
    "CoreActionRowsJson",
    "CoreEventSnapshotRowsJson",
    "runtime.assert_bridge_call requires appId and method",
    "assertion_failed",
    "Expected bridge call was not recorded",
    "console_errors_found",
    "Console error logs were found",
    "DELETE FROM bridge_calls WHERE app_id = ?",
    "DELETE FROM bridge_calls",
    "DELETE FROM core_actions WHERE app_id = ?",
    "DELETE FROM core_actions",
    "DELETE FROM core_events WHERE app_id = ?",
    "DELETE FROM core_events",
    "WHERE method = 'app.log'",
    "WHERE method = 'notification.toast'",
    "level.value() == L\"error\"",
    "RawJsonOrNull",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows resource/log control source should contain ${snippet}`);
  }
});
