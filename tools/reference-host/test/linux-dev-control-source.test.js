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
  ]) {
    assert.equal(header.includes(snippet), true, `dev control header should expose ${snippet}`);
  }

  for (const snippet of [
    "#ifndef NDEBUG",
    "NATIVE_AI_LINUX_DEV_CONTROL",
    "--native-ai-dev-control",
    "--control-plane-port",
    "dev_control_plane_start(&config",
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
  assert.equal(meson.includes("libsoup-3.0"), true);
});
