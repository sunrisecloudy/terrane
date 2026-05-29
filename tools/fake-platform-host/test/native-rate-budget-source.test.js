import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native bridges enforce bridge and network rate budgets", () => {
  const iosBridge = read("native/ios/Sources/NativeAIHostIOS/WebBridge.swift");
  const macosBridge = read("native/macos/Sources/NativeAIHostMac/WebBridge.swift");
  const macosControl = read("native/macos/Sources/NativeAIHostMac/DevControlPlane.swift");
  const androidBridge = read("native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt");
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsBridgeHeader = read("native/windows/src/WebBridge.h");
  const linuxBridge = read("native/linux/src/web_bridge.c");

  for (const [target, source] of [
    ["ios", iosBridge],
    ["macos", macosBridge],
    ["macos control", macosControl],
  ]) {
    assert.match(source, /bridgeRateBudgetFailure\(_ request: BridgeRequest\)/, `${target} has a rate budget hook`);
    assert.match(source, /maxBridgeCallsPerMinute/, `${target} checks bridge-call rate`);
    assert.match(source, /maxNetworkRequestsPerMinute/, `${target} checks network-call rate`);
    assert.match(source, /Bridge call rate exceeds manifest\.resourceBudget\.maxBridgeCallsPerMinute/, `${target} reports bridge budget`);
    assert.match(source, /Network request rate exceeds manifest\.resourceBudget\.maxNetworkRequestsPerMinute/, `${target} reports network budget`);
  }

  assert.match(androidBridge, /private fun bridgeRateBudgetFailure\(request: BridgeRequest\): String\?/);
  assert.match(androidBridge, /request\.context\.resourceBudget\.optInt\("maxBridgeCallsPerMinute", -1\)/);
  assert.match(androidBridge, /request\.context\.resourceBudget\.optInt\("maxNetworkRequestsPerMinute", -1\)/);
  assert.match(androidBridge, /Bridge call rate exceeds manifest\.resourceBudget\.maxBridgeCallsPerMinute/);
  assert.match(androidBridge, /Network request rate exceeds manifest\.resourceBudget\.maxNetworkRequestsPerMinute/);

  assert.match(windowsBridgeHeader, /ResourceBudgetFailure\(BridgeRequest const& request\) const/);
  assert.match(windowsBridgeHeader, /BridgeCallCountSince\(std::wstring const& appId, int seconds\) const/);
  assert.match(windowsBridge, /WebBridge::ResourceBudgetFailure\(BridgeRequest const& request\) const/);
  assert.match(windowsBridge, /request\.context\.resourceBudget\.find\(L"maxBridgeCallsPerMinute"\)/);
  assert.match(windowsBridge, /request\.context\.resourceBudget\.find\(L"maxNetworkRequestsPerMinute"\)/);
  assert.match(windowsBridge, /L"Bridge call rate exceeds manifest\.resourceBudget\.maxBridgeCallsPerMinute"/);
  assert.match(windowsBridge, /L"Network request rate exceeds manifest\.resourceBudget\.maxNetworkRequestsPerMinute"/);

  assert.match(linuxBridge, /static JsonNode \*resource_budget_failure\(WebBridge \*bridge, const BridgeRequest \*request\)/);
  assert.match(linuxBridge, /resource_budget_limit\(&request->context, "maxBridgeCallsPerMinute", &limit\)/);
  assert.match(linuxBridge, /resource_budget_limit\(&request->context, "maxNetworkRequestsPerMinute", &limit\)/);
  assert.match(linuxBridge, /Bridge call rate exceeds manifest\.resourceBudget\.maxBridgeCallsPerMinute/);
  assert.match(linuxBridge, /Network request rate exceeds manifest\.resourceBudget\.maxNetworkRequestsPerMinute/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
