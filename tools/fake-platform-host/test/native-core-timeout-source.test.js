import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("macOS core.step enforces timeout without blocking the WebView reply path", () => {
  const macosBridge = read("native/macos/Sources/NativeAIHostMac/WebBridge.swift");
  const macosCore = read("native/macos/Sources/NativeAIHostMac/ZigCoreBridge.swift");
  const macosTests = read("native/macos/Tests/NativeAIHostMacTests/NativeHostTests.swift");

  assert.match(macosBridge, /if request\.method == "core\.step" \{/);
  assert.match(macosBridge, /core\.stepAsync\(request\)/);
  assert.match(macosBridge, /replyHandler\(result\.asDictionary\(\), nil\)/);
  assert.match(macosCore, /stepTimeoutMilliseconds: Int = 2_000/);
  assert.match(macosCore, /DispatchQueue\.global\(qos: \.userInitiated\)\.asyncAfter\(deadline: \.now\(\) \+ \.milliseconds\(stepTimeoutMilliseconds\)\)/);
  assert.match(macosCore, /code: "timeout"/);
  assert.match(macosCore, /"timeoutMs": stepTimeoutMilliseconds/);
  assert.match(macosTests, /coreStepReturnsTimeoutWhenZigCoreExceedsHostTimeout/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
