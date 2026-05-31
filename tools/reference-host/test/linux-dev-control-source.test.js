import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

test("Linux dev control plane is debug-only, loopback-bound, token-gated, and audited", () => {
  const main = read("native/linux/src/main.c");
  const control = read("native/linux/src/dev_control_plane.c");
  const header = read("native/linux/src/dev_control_plane.h");
  const meson = read("native/linux/meson.build");

  for (const snippet of [
    "DevControlPlaneConfig",
    "dev_control_plane_start",
    "dev_control_plane_stop",
    "dev_control_plane_port",
    "dev_control_plane_token_path",
    "dev_control_plane_set_bridge",
  ]) {
    assert.equal(header.includes(snippet), true, `dev control header should expose ${snippet}`);
  }

  for (const snippet of [
    "#ifndef NDEBUG",
    "NATIVE_AI_LINUX_DEV_CONTROL",
    "--native-ai-dev-control",
    "--control-plane-port",
    "dev_control_plane_start(&config",
    "dev_control_plane_set_bridge(dev_control",
    "Linux dev control plane is disabled in release builds",
  ]) {
    assert.equal(main.includes(snippet), true, `Linux main should contain ${snippet}`);
  }

  for (const snippet of [
    "#ifndef NDEBUG",
    "g_inet_address_new_loopback(G_SOCKET_FAMILY_IPV4)",
    "soup_server_listen(plane->server, socket_address",
    'g_getenv("XDG_RUNTIME_DIR")',
    '"native-ai-webapp", "control.token"',
    "g_open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600)",
    "g_base64_encode(bytes, sizeof(bytes))",
    "X-Platform-Control-Token",
    "control_auth_required",
    "SOUP_STATUS_UNAUTHORIZED",
    "soup_server_add_handler(plane->server, \"/health\"",
    "control_route_handler",
    "POST",
    "session_create_handler",
    "session_snapshot_handler",
    "session_events_handler",
    "session_command_handler",
    "control.sessions.create",
    "control.sessions.snapshot",
    "control.sessions.events",
    "runtime.call_bridge",
    "runtime.core_step",
    "runtime.core_snapshot",
    "runtime.replay_events",
    "runtime.assert_core_action",
    "platform.list_targets",
    "platform.list_webapps",
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
    "runtime.bridge_calls",
    "runtime.clear_logs",
    "runtime.notification_capture",
    "runtime.assert_bridge_call",
    "runtime.assert_no_console_errors",
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "runtime.assert_storage",
    "runtime.network_mock_set",
    "runtime.network_mock_reset",
    "runtime.dialog_mock_set",
    "db.snapshot",
    "db.query_app_storage",
    "db.query_app_versions",
    "db.query_bridge_calls",
    "db.query_core_events",
    "db.query_test_runs",
    "safe_table_rows_json",
    "db_snapshot_json",
    "db_query_rows_json",
    "Unsupported DB inspection command",
    "control_call_bridge",
    "control_core_step",
    "runtime_resource_usage_json",
    "runtime_event_log_json",
    "runtime_console_logs_json",
    "web_bridge_handle_json",
    "app_sandbox_context_for_app",
    "core.step",
    "unsupported_tool",
    "UPDATE control_sessions SET status = 'ended'",
    "health_result_json",
    "platform_list_targets_json",
    "platform_list_webapps_json",
    "control_sessions",
    "control_commands",
    "platform.health",
    "NATIVE_AI_LINUX_CONTROL_READY port=",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux dev control source should contain ${snippet}`);
  }

  assert.equal(meson.includes("'src/dev_control_plane.c'"), true);
  assert.equal(meson.includes("'src/app_sandbox.c'"), true);
  assert.equal(meson.includes("libsoup-3.0"), true);
});

test("Linux dev control exposes target and webapp listing controls", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "platform.list_targets",
    "platform.list_webapps",
    "platform_list_targets_json",
    "platform_list_webapps_json",
    "\"linux-native\"",
    "\"available\"",
    "includeUninstalled",
    "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version",
    "LEFT JOIN app_versions v ON v.install_id = a.active_install_id",
    "app_sandbox_manifest_path_for_app",
    "append_bundled_webapp",
    "notes-lite",
    "task-workbench",
    "api-dashboard",
    "bundled",
    "installed",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux list control source should contain ${snippet}`);
  }
});

test("Linux dev control supports direct storage get, set, reset, and assertions", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.storage_reset",
    "platform.reset_webapp",
    "runtime.assert_storage",
    "storage_command_args",
    "runtime.storage_get requires appId and key",
    "runtime.storage_set requires appId, key, and value",
    "runtime.assert_storage requires appId, key, and value",
    "runtime_storage_bridge_json",
    "control_storage_get",
    "control_storage_set",
    "storage.get",
    "storage.set",
    "stored_storage_value_json",
    "runtime_assert_storage_json",
    "Expected storage key was not found",
    "Storage value did not match expected value",
    "object_boolean_true(args, \"confirm\")",
    "confirmation_required",
    "Storage reset command requires confirm: true",
    "runtime_storage_reset_json",
    "INSERT INTO runtime_snapshots",
    "delete_rows_for_app(db, \"app_storage\"",
    "delete_rows_for_app(db, \"bridge_calls\"",
    "delete_rows_for_app(db, \"core_events\"",
    "clearedStorageKeys",
    "storageRowsDeleted",
    "clearedBridgeCalls",
    "clearedCoreEvents",
    "g_compute_checksum_for_string(G_CHECKSUM_SHA256",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux storage control source should contain ${snippet}`);
  }
});

test("Linux dev control exposes DB-backed network and dialog effect mocks", () => {
  const control = read("native/linux/src/dev_control_plane.c");
  const network = read("native/linux/src/platform_network.c");
  const dialogs = read("native/linux/src/platform_dialogs.c");
  const bridge = read("native/linux/src/web_bridge.c");

  for (const snippet of [
    "runtime.network_mock_set",
    "runtime.network_mock_reset",
    "runtime.dialog_mock_set",
    "runtime.network_mock_set requires urlPattern or match.url and response",
    "runtime.dialog_mock_set requires dialogType or method",
    "runtime_network_mock_set_json",
    "runtime_network_mock_reset_json",
    "runtime_dialog_mock_set_json",
    "INSERT INTO network_mocks",
    "DELETE FROM network_mocks WHERE app_id = ?",
    "INSERT INTO dialog_mocks",
    "Runtime effect mock command requires args object",
    "Runtime effect mock appId is not a valid generated app id",
    "control_session_allows_app",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux effect mock control source should contain ${snippet}`);
  }

  for (const snippet of [
    "SELECT response_json, url_pattern FROM network_mocks",
    "url_matches",
    "mocked_network_response",
    "delayMs",
    "mock_payload_without_delay",
    "network.response exceeds manifest.networkPolicy maxResponseBytes",
  ]) {
    assert.equal(network.includes(snippet), true, `Linux network mock source should contain ${snippet}`);
  }

  for (const snippet of [
    "SELECT response_json FROM dialog_mocks",
    "stored_dialog_mock",
    "runtime_session_id_for_request",
    "bridge_success(request, mock)",
  ]) {
    assert.equal(dialogs.includes(snippet), true, `Linux dialog mock source should contain ${snippet}`);
  }

  assert.equal(bridge.includes("bridge->network.db = bridge->storage == NULL ? NULL : bridge->storage->db"), true);
  assert.equal(bridge.includes("platform_dialogs_init(&bridge->dialogs, owner_window, bridge->storage == NULL ? NULL : bridge->storage->db)"), true);
});

test("Linux dev control exposes DB-backed runtime resource and log inspection commands", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
    "Runtime inspection command requires args object",
    "runtime.resource_usage requires appId",
    "runtime_resource_usage_json",
    "runtime_event_log_json",
    "runtime_console_logs_json",
    "append_console_log_rows",
    "storageBytes",
    "bridgeCalls",
    "coreEvents",
    "networkRequestsLastMinute",
    "logLinesLastMinute",
    "WHERE method = 'app.log'",
    "append_bridge_call_rows(builder, db, app_id)",
    "append_core_event_rows(builder, db, app_id)",
    "control_session_allows_app",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux runtime inspection source should contain ${snippet}`);
  }
});

test("Linux dev control exposes bridge log assertions and notification capture", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "runtime.bridge_calls",
    "runtime.clear_logs",
    "runtime.notification_capture",
    "runtime.assert_bridge_call",
    "runtime.assert_no_console_errors",
    "runtime_bridge_calls_json",
    "clear_runtime_logs_json",
    "notification_capture_json",
    "assert_bridge_call_json",
    "assert_no_console_errors_json",
    "append_bridge_call_row_object",
    "append_notification_rows",
    "console_log_row_is_error",
    "Runtime bridge log command requires args object",
    "Runtime bridge log appId must be a string",
    "Runtime bridge log appId is not a valid generated app id",
    "runtime.assert_bridge_call requires appId and method",
    "Expected bridge call was not recorded",
    "Console error logs were found",
    "DELETE FROM %s WHERE app_id = ?",
    "delete_runtime_log_rows(db, \"bridge_calls\"",
    "delete_runtime_log_rows(db, \"core_actions\"",
    "delete_runtime_log_rows(db, \"core_events\"",
    "WHERE method = 'notification.toast'",
    "WHERE method = 'app.log'",
    "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux bridge log source should contain ${snippet}`);
  }
});

test("Linux dev control supports core replay, snapshots, and action assertions", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "runtime.core_snapshot",
    "runtime.replay_events",
    "runtime.assert_core_action",
    "runtime_core_snapshot_json",
    "runtime_replay_events_json",
    "runtime_assert_core_action_json",
    "runtime.core_snapshot requires appId",
    "runtime.replay_events requires appId",
    "runtime.replay_events events must be an array",
    "runtime.assert_core_action requires appId",
    "runtime.assert_core_action type must be a string",
    "runtime.assert_core_action match must be an object",
    "core_action.not_found",
    "Expected core action was not found",
    "json_node_matches_subset",
    "append_core_event_snapshot_rows",
    "append_core_action_rows",
    "core_state_version",
    "SELECT action_json FROM core_actions WHERE app_id = ? ORDER BY created_at",
    "ZigCoreBridge replay_core",
    "zig_core_bridge_init(&replay_core)",
    "zig_core_bridge_step(&replay_core",
    "control_replay_",
    "linux-control-replay",
  ]) {
    assert.equal(control.includes(snippet), true, `Linux core control source should contain ${snippet}`);
  }
});

test("Linux dev control database inspection uses fixed allowlisted queries only", () => {
  const control = read("native/linux/src/dev_control_plane.c");

  for (const snippet of [
    "sqlite3_column_type",
    "safe_db_apps",
    "safe_db_app_storage",
    "safe_db_app_versions",
    "safe_db_bridge_calls",
    "safe_db_core_events",
    "safe_db_test_runs",
    "filter_column",
    "filter_value",
    "LIMIT 100",
    'db_tool_requires_app_id(tool)',
  ]) {
    assert.equal(control.includes(snippet), true, `Linux DB control source should contain ${snippet}`);
  }

  for (const forbidden of [
    "db.query_sql",
    "SELECT *",
    'object_string(args, "sql"',
  ]) {
    assert.equal(control.includes(forbidden), false, `Linux DB control source must not contain ${forbidden}`);
  }
});
