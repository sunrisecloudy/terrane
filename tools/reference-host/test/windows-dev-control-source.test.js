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
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
    "ResourceUsageJson",
    "EventLogJson",
    "ConsoleLogsJson",
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
    "BridgeCallRowsJson(db, appId)",
    "CoreEventRowsJson(db, appId)",
    "WHERE method = 'app.log'",
    "RawJsonOrNull",
  ]) {
    assert.equal(control.includes(snippet), true, `Windows resource/log control source should contain ${snippet}`);
  }
});
