import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("macOS core.step enforces timeout without blocking the WebView reply path", () => {
  const macosBridge = read("native/macos/Sources/TerraneHostMac/WebBridge.swift");
  const macosCore = read("native/macos/Sources/TerraneHostMac/ForgeCoreBridge.swift");
  const macosTests = read("native/macos/Tests/TerraneHostMacTests/NativeHostTests.swift");

  assert.match(macosBridge, /if request\.method == "core\.step" \{/);
  assert.match(macosBridge, /core\.stepAsync\(request\)/);
  assert.match(macosBridge, /replyHandler\(result\.asDictionary\(\), nil\)/);
  assert.match(macosCore, /stepTimeoutMilliseconds: Int = 2_000/);
  assert.match(macosCore, /DispatchQueue\.global\(qos: \.userInitiated\)\.asyncAfter\(deadline: \.now\(\) \+ \.milliseconds\(stepTimeoutMilliseconds\)\)/);
  assert.match(macosCore, /terrane_forge_core_handle_command/);
  assert.match(macosCore, /name: "legacy\.core_step"/);
  assert.match(macosCore, /code: "timeout"/);
  assert.match(macosCore, /"timeoutMs": stepTimeoutMilliseconds/);
  assert.match(macosTests, /coreStepReturnsTimeoutWhenForgeCoreExceedsHostTimeout/);
});

test("Windows core.step enforces a structured host timeout around Forge FFI calls", () => {
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsCore = read("native/windows/src/ForgeCoreBridge.cpp");
  const windowsCoreHeader = read("native/windows/src/ForgeCoreBridge.h");
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
  assert.match(windowsCore, /forge_core_handle_command/);
  assert.match(windowsCore, /L"legacy\.core_step"/);
  assert.match(windowsCoreHeader, /std::shared_ptr<CoreRuntime> runtime_/);
  assert.match(windowsBridge, /void WebBridge::HandleJsonAsync/);
  assert.match(windowsBridge, /request\.method == L"core\.step"/);
  assert.match(windowsBridge, /core_\.StepAsync\(request/);
  assert.match(windowsHost, /bridge_->HandleJsonAsync/);
  assert.match(windowsHost, /PostMessageW\(window_, kAsyncBridgeResponseMessage/);
  assert.match(windowsHostHeader, /TryHandleWindowMessage/);
  assert.doesNotMatch(windowsCore, /core_step_json/);
  assert.doesNotMatch(windowsHost, /response = context\.has_value\(\)\s*\?\s*bridge_->HandleJson\(requestJson, context\.value\(\)\)/);
});

test("Linux core.step loader prefers packaged libforge_ffi beside the executable", () => {
  const linuxCore = read("native/linux/src/forge_core_bridge.c");

  assert.match(linuxCore, /g_getenv\("TERRANE_FORGE_FFI_SO"\)/);
  assert.match(linuxCore, /g_file_read_link\("\/proc\/self\/exe"/);
  assert.match(linuxCore, /g_path_get_dirname/);
  assert.match(linuxCore, /g_build_filename\(dir,\s*"libforge_ffi\.so",\s*NULL\)/);
  assert.match(linuxCore, /dlopen\(path, RTLD_NOW \| RTLD_LOCAL\)/);
  assert.match(linuxCore, /dlsym\(handle, "forge_core_handle_command"\)/);
  assert.match(linuxCore, /json_builder_add_string_value\(builder, "legacy\.core_step"\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
