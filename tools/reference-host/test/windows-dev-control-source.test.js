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
