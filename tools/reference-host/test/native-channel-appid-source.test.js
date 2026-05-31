import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native bridges reject appId in bridge params before dispatch", () => {
  const targets = [
    {
      label: "macOS",
      path: "native/macos/Sources/NativeAIHostMac/WebBridge.swift",
      snippets: [
        'request.params["appId"] != nil',
        'message: "Bridge params must not include appId; app id is channel-derived"',
        'details: ["field": "appId"]',
      ],
    },
    {
      label: "iOS",
      path: "native/ios/Sources/NativeAIHostIOS/WebBridge.swift",
      snippets: [
        'request.params["appId"] != nil',
        'message: "Bridge params must not include appId; app id is channel-derived"',
        'details: ["field": "appId"]',
      ],
    },
    {
      label: "Android",
      path: "native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt",
      snippets: [
        'request.params.has("appId")',
        '"Bridge params must not include appId; app id is channel-derived"',
        'JSONObject(mapOf("field" to "appId"))',
      ],
    },
    {
      label: "Windows",
      path: "native/windows/src/WebBridge.cpp",
      snippets: [
        'request.params.HasKey(L"appId")',
        'L"Bridge params must not include appId; app id is channel-derived"',
        'details.Insert(L"field", json::JsonValue::CreateStringValue(L"appId"))',
      ],
    },
    {
      label: "Linux",
      path: "native/linux/src/web_bridge.c",
      snippets: [
        'json_object_has_member(request.params, "appId")',
        '"Bridge params must not include appId; app id is channel-derived"',
        'json_object_set_string_member(details, "field", "appId")',
      ],
    },
  ];

  for (const target of targets) {
    const source = fs.readFileSync(path.join(repoRoot, target.path), "utf8");
    for (const snippet of target.snippets) {
      assert.equal(source.includes(snippet), true, `${target.label} bridge must contain ${snippet}`);
    }
  }
});

test("Windows WebView2 bridge cross-checks runtime envelopes against host-owned mounts", () => {
  const host = fs.readFileSync(path.join(repoRoot, "native/windows/src/WebViewHost.cpp"), "utf8");
  const header = fs.readFileSync(path.join(repoRoot, "native/windows/src/WebViewHost.h"), "utf8");
  const runtime = fs.readFileSync(path.join(repoRoot, "runtime-web/runtime.js"), "utf8");

  assert.match(header, /registeredMountsByToken_/);
  assert.match(header, /SandboxContextForRegisteredMount/);
  assert.match(host, /IsRuntimeMountRequest/);
  assert.match(host, /CreateHostOwnedRuntimeMount/);
  assert.match(host, /NewRuntimeMountToken/);
  assert.match(host, /RegisterHostOwnedRuntimeMount/);
  assert.match(host, /registeredMountsByToken_\.clear\(\)/);
  assert.match(host, /registeredMountsByToken_\[mountToken\] = appId/);
  assert.match(host, /auto context = SandboxContextForRegisteredMount\(appId, mountToken\)/);
  assert.match(host, /bridge_->HandleJsonAsync\(/);
  assert.match(host, /Runtime bridge envelope does not match a host-owned mount channel/);
  assert.doesNotMatch(host, /bridge_->HandleJson\(requestJson, SandboxContextForApp\(appId, mountToken\)\)/);
  assert.doesNotMatch(host, /response = context\.has_value\(\)\s*\?\s*bridge_->HandleJson\(requestJson, context\.value\(\)\)/);
  assert.match(host, /RegisterHostOwnedRuntimeMount\(appId, L"windows-webview-smoke"\)/);
  assert.doesNotMatch(host, /IsRuntimeMountRegistration/);

  assert.match(runtime, /requestWebView2RuntimeMountToken\(app\.id\)/);
  assert.match(runtime, /type: "runtime\.mount_request"/);
  assert.match(runtime, /type === "runtime\.mount_response"/);
  assert.doesNotMatch(runtime, /runtime\.mount_registered/);
});
