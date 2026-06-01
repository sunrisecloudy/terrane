import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("macOS core.step enforces timeout without blocking the WebView reply path", () => {
  const macosBridge = read("native/macos/Sources/TerraneHostMac/WebBridge.swift");
  const macosCore = read("native/macos/Sources/TerraneHostMac/ZigCoreBridge.swift");
  const macosTests = read("native/macos/Tests/TerraneHostMacTests/NativeHostTests.swift");

  assert.match(macosBridge, /if request\.method == "core\.step" \{/);
  assert.match(macosBridge, /core\.stepAsync\(request\)/);
  assert.match(macosBridge, /replyHandler\(result\.asDictionary\(\), nil\)/);
  assert.match(macosCore, /stepTimeoutMilliseconds: Int = 2_000/);
  assert.match(macosCore, /DispatchQueue\.global\(qos: \.userInitiated\)\.asyncAfter\(deadline: \.now\(\) \+ \.milliseconds\(stepTimeoutMilliseconds\)\)/);
  assert.match(macosCore, /code: "timeout"/);
  assert.match(macosCore, /"timeoutMs": stepTimeoutMilliseconds/);
  assert.match(macosTests, /coreStepReturnsTimeoutWhenZigCoreExceedsHostTimeout/);
});

test("Windows core.step enforces a structured host timeout around Zig DLL calls", () => {
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsCore = read("native/windows/src/ZigCoreBridge.cpp");
  const windowsCoreHeader = read("native/windows/src/ZigCoreBridge.h");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const windowsHostHeader = read("native/windows/src/WebViewHost.h");

  assert.match(windowsCore, /constexpr uint32_t kCoreStepTimeoutMs = 2000/);
  assert.match(windowsCoreHeader, /void StepAsync\(BridgeRequest request, StepCompletion completion\)/);
  assert.match(windowsCore, /std::thread\(/);
  assert.match(windowsCore, /\.detach\(\)/);
  assert.match(windowsCore, /std::this_thread::sleep_for\(std::chrono::milliseconds\(kCoreStepTimeoutMs\)\)/);
  assert.match(windowsCore, /CompleteStep\(state, TimeoutFailure\(request\)\)/);
  assert.match(windowsCore, /std::lock_guard<std::mutex> guard\(runtime->stepMutex\)/);
  assert.match(windowsCore, /BridgeResponse::Failure\(request\.id, request\.hasId, L"timeout", L"core\.step timed out", details\)/);
  assert.match(windowsCore, /details\.Insert\(L"timeoutMs", json::JsonValue::CreateNumberValue\(kCoreStepTimeoutMs\)\)/);
  assert.match(windowsCoreHeader, /std::shared_ptr<CoreRuntime> runtime_/);
  assert.match(windowsBridge, /void WebBridge::HandleJsonAsync/);
  assert.match(windowsBridge, /request\.method == L"core\.step"/);
  assert.match(windowsBridge, /core_\.StepAsync\(request/);
  assert.match(windowsHost, /bridge_->HandleJsonAsync/);
  assert.match(windowsHost, /PostMessageW\(window_, kAsyncBridgeResponseMessage/);
  assert.match(windowsHostHeader, /TryHandleWindowMessage/);
  assert.doesNotMatch(windowsCore, /int32_t code = stepJson_\(core_/);
  assert.doesNotMatch(windowsHost, /response = context\.has_value\(\)\s*\?\s*bridge_->HandleJson\(requestJson, context\.value\(\)\)/);
});

test("Linux core.step loader prefers packaged libzig_core beside the executable", () => {
  const linuxCore = read("native/linux/src/zig_core_bridge.c");

  assert.match(linuxCore, /g_getenv\("TERRANE_ZIG_CORE_SO"\)/);
  assert.match(linuxCore, /g_file_read_link\("\/proc\/self\/exe"/);
  assert.match(linuxCore, /g_path_get_dirname/);
  assert.match(linuxCore, /g_build_filename\(dir,\s*"libzig_core\.so",\s*NULL\)/);
  assert.match(linuxCore, /dlopen\(path, RTLD_NOW \| RTLD_LOCAL\)/);
  assert.match(linuxCore, /dlsym\(handle, "core_step_json"\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
