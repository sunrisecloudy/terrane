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

test("Linux core.step loader prefers packaged libzig_core beside the executable", () => {
  const linuxCore = read("native/linux/src/zig_core_bridge.c");

  assert.match(linuxCore, /g_getenv\("NATIVE_AI_ZIG_CORE_SO"\)/);
  assert.match(linuxCore, /g_file_read_link\("\/proc\/self\/exe"/);
  assert.match(linuxCore, /g_path_get_dirname/);
  assert.match(linuxCore, /g_build_filename\(dir,\s*"libzig_core\.so",\s*NULL\)/);
  assert.match(linuxCore, /dlopen\(path, RTLD_NOW \| RTLD_LOCAL\)/);
  assert.match(linuxCore, /dlsym\(handle, "core_step_json"\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
