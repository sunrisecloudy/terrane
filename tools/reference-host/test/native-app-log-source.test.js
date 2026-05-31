import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native bridges validate and budget app.log", () => {
  const androidBridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");
  const androidHost = read("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");
  const iosBridge = read("native/ios/Sources/NativeAIHostIOS/WebBridge.swift");
  const macosBridge = read("native/macos/Sources/NativeAIHostMac/WebBridge.swift");
  const macosControl = read("native/macos/Sources/NativeAIHostMac/DevControlPlane.swift");
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxBridge = read("native/linux/src/web_bridge.c");
  const linuxHost = read("native/linux/src/webkit_host.c");
  const linuxSandbox = read("native/linux/src/app_sandbox.c");

  assert.match(androidBridge, /private fun appLog\(request: BridgeRequest\): String/);
  assert.match(androidBridge, /app\.log level must be debug, info, warn, or error/);
  assert.match(androidBridge, /app\.log requires message/);
  assert.match(androidBridge, /resource_budget_exceeded/);
  assert.match(androidBridge, /maxLogLinesPerMinute/);
  assert.match(androidBridge, /bridgeCallCount\(request\.context\.appId, "app\.log", seconds = 60\)/);
  assert.match(androidBridge, /"limits" to JSONObject\(/);
  assert.match(androidBridge, /request\.context\.resourceBudget\.toMap\(\)/);
  assert.match(androidHost, /resourceBudget = manifest\.optJSONObject\("resourceBudget"\) \?: JSONObject\(\)/);

  assert.match(iosBridge, /private func appLog\(_ request: BridgeRequest\) -> BridgeResponse/);
  assert.match(iosBridge, /app\.log level must be debug, info, warn, or error/);
  assert.match(iosBridge, /app\.log requires message/);
  assert.match(iosBridge, /resource_budget_exceeded/);
  assert.match(iosBridge, /maxLogLinesPerMinute/);
  assert.match(iosBridge, /bridgeCallCount\(appId: request\.context\.appId, method: "app\.log", seconds: 60\)/);
  assert.match(iosBridge, /for \(key, value\) in request\.context\.resourceBudget/);
  assert.match(iosBridge, /let resourceBudget: \[String: Int\]/);

  assert.match(macosBridge, /private func appLog\(_ request: BridgeRequest\) -> BridgeResponse/);
  assert.match(macosBridge, /app\.log level must be debug, info, warn, or error/);
  assert.match(macosBridge, /app\.log requires message/);
  assert.match(macosBridge, /resource_budget_exceeded/);
  assert.match(macosBridge, /maxLogLinesPerMinute/);
  assert.match(macosBridge, /bridgeCallCount\(appId: request\.context\.appId, method: "app\.log", seconds: 60\)/);
  assert.match(macosBridge, /for \(key, value\) in request\.context\.resourceBudget/);
  assert.match(macosBridge, /let resourceBudget: \[String: Int\]/);
  assert.match(macosBridge, /metadata_json\)\n\s+VALUES \(\?, 'macos', 'macos', '0\.1\.0'/);
  assert.match(macosControl, /resourceBudget: AppSandboxContext\.resourceBudget\(from: manifest\)/);

  assert.match(windowsBridge, /json::JsonObject WebBridge::AppLog\(BridgeRequest const& request\) const/);
  assert.match(windowsBridge, /app\.log level must be debug, info, warn, or error/);
  assert.match(windowsBridge, /app\.log requires message/);
  assert.match(windowsBridge, /resource_budget_exceeded/);
  assert.match(windowsBridge, /maxLogLinesPerMinute/);
  assert.match(windowsBridge, /BridgeCallCountSince\(request\.context\.appId, L"app\.log", 60\)/);
  assert.match(windowsBridge, /for \(auto const& \[key, value\] : request\.context\.resourceBudget\)/);
  assert.match(windowsHost, /\.resourceBudget = ResourceBudgetForApp\(appId\)/);

  assert.match(linuxBridge, /static JsonNode \*app_log_response\(WebBridge \*bridge, const BridgeRequest \*request\)/);
  assert.match(linuxBridge, /app\.log level must be debug, info, warn, or error/);
  assert.match(linuxBridge, /app\.log requires message/);
  assert.match(linuxBridge, /resource_budget_exceeded/);
  assert.match(linuxBridge, /maxLogLinesPerMinute/);
  assert.match(linuxBridge, /bridge_call_count_since\(bridge, request->context\.app_id, "app\.log", 60\)/);
  assert.match(linuxBridge, /add_resource_budget_limits\(builder, &request->context\)/);
  assert.match(linuxSandbox, /\.resource_budget = resource_budget_for_app\(app_id\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
