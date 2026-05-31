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
    "runtime.resource_usage",
    "runtime.event_log",
    "runtime.console_logs",
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
