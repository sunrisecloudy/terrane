import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("mobile native bridges validate and budget app.log", () => {
  const androidBridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");
  const androidHost = read("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");
  const iosBridge = read("native/ios/Sources/NativeAIHostIOS/WebBridge.swift");

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
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
