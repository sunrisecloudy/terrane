import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Windows and Linux native smoke suites include WebView bridge-message coverage", () => {
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxHost = read("native/linux/src/webkit_host.c");
  const windowsSmoke = read("tools/fake-platform-host/test/windows-native-build.test.js");
  const linuxSmoke = read("tools/fake-platform-host/test/linux-native-build.test.js");

  assert.match(windowsHost, /window\.chrome\.webview\.postMessage\(JSON\.stringify/);
  assert.match(windowsHost, /windows_smoke_bridge_storage_set/);
  assert.match(windowsHost, /windows_smoke_bridge_storage_get/);
  assert.match(windowsHost, /windows_smoke_bridge_core_step/);
  assert.match(windowsSmoke, /NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_SET_OK/);
  assert.match(windowsSmoke, /NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_GET_OK/);
  assert.match(windowsSmoke, /NATIVE_AI_WINDOWS_SMOKE_BRIDGE_CORE_STEP_OK/);

  assert.match(linuxHost, /messageHandlers && window\.webkit\.messageHandlers\.NativeAIPlatformBridge/);
  assert.match(linuxHost, /linux_smoke_bridge_storage_set/);
  assert.match(linuxHost, /linux_smoke_bridge_storage_get/);
  assert.match(linuxHost, /linux_smoke_bridge_core_step/);
  assert.match(linuxSmoke, /NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK/);
  assert.match(linuxSmoke, /NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK/);
  assert.match(linuxSmoke, /NATIVE_AI_LINUX_SMOKE_BRIDGE_CORE_STEP_OK/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
