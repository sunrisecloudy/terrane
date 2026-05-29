import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native bridges persist bridge and core logs", () => {
  const androidBridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");
  const androidHost = read("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");
  const iosBridge = read("native/ios/Sources/NativeAIHostIOS/WebBridge.swift");
  const iosHost = read("native/ios/Sources/NativeAIHostIOS/WebHostView.swift");
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxBridge = read("native/linux/src/web_bridge.c");
  const linuxHost = read("native/linux/src/webkit_host.c");

  assert.match(androidBridge, /"runtime_sessions"/);
  assert.match(androidBridge, /"bridge_calls"/);
  assert.match(androidBridge, /"core_events"/);
  assert.match(androidBridge, /"core_actions"/);
  assert.match(androidBridge, /"params_json"/);
  assert.match(androidBridge, /"result_json"/);
  assert.match(androidBridge, /"error_json"/);
  assert.match(androidHost, /rowCount\(db, "bridge_calls", appId, "core.step"\)/);
  assert.match(androidHost, /rowCount\(db, "core_events", appId\)/);
  assert.match(androidHost, /rowCount\(db, "core_actions", appId\)/);
  assert.match(androidHost, /core smoke did not persist bridge\/core log rows/);

  assert.match(iosBridge, /INSERT INTO runtime_sessions/);
  assert.match(iosBridge, /INSERT INTO bridge_calls/);
  assert.match(iosBridge, /INSERT INTO core_events/);
  assert.match(iosBridge, /INSERT INTO core_actions/);
  assert.match(iosBridge, /params_json/);
  assert.match(iosBridge, /result_json/);
  assert.match(iosBridge, /error_json/);
  assert.match(iosHost, /rowCount\(db: db, table: "bridge_calls", appId: appId, method: "core.step"\)/);
  assert.match(iosHost, /rowCount\(db: db, table: "core_events", appId: appId\)/);
  assert.match(iosHost, /rowCount\(db: db, table: "core_actions", appId: appId\)/);
  assert.match(iosHost, /SELECT COUNT\(\*\) FROM \\\(table\) WHERE app_id = \? AND method = \?/);
  assert.match(iosHost, /core smoke did not persist bridge\/core log rows/);

  assert.match(windowsBridge, /INSERT INTO runtime_sessions/);
  assert.match(windowsBridge, /INSERT INTO bridge_calls/);
  assert.match(windowsBridge, /INSERT INTO core_events/);
  assert.match(windowsBridge, /INSERT INTO core_actions/);
  assert.match(windowsBridge, /params_json/);
  assert.match(windowsBridge, /result_json/);
  assert.match(windowsBridge, /error_json/);
  assert.match(windowsHost, /SELECT COUNT\(\*\) FROM bridge_calls WHERE app_id = \? AND method = \?/);
  assert.match(windowsHost, /SELECT COUNT\(\*\) FROM core_events WHERE app_id = \?/);
  assert.match(windowsHost, /SELECT COUNT\(\*\) FROM core_actions WHERE app_id = \?/);
  assert.match(windowsHost, /fixed bridge surface smoke did not persist bridge_calls rows/);
  assert.match(windowsHost, /core smoke did not persist core_events\/core_actions rows/);

  assert.match(linuxBridge, /INSERT INTO runtime_sessions/);
  assert.match(linuxBridge, /INSERT INTO bridge_calls/);
  assert.match(linuxBridge, /INSERT INTO core_events/);
  assert.match(linuxBridge, /INSERT INTO core_actions/);
  assert.match(linuxBridge, /params_json/);
  assert.match(linuxBridge, /result_json/);
  assert.match(linuxBridge, /error_json/);
  assert.match(linuxHost, /SELECT COUNT\(\*\) FROM bridge_calls WHERE app_id = \? AND method = \?/);
  assert.match(linuxHost, /SELECT COUNT\(\*\) FROM core_events WHERE app_id = \?/);
  assert.match(linuxHost, /SELECT COUNT\(\*\) FROM core_actions WHERE app_id = \?/);
  assert.match(linuxHost, /fixed bridge surface smoke did not persist bridge_calls rows/);
  assert.match(linuxHost, /core smoke did not persist core_events\/core_actions rows/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
