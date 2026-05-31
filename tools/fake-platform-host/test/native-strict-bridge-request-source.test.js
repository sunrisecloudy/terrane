import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native bridges enforce strict request and runtime-envelope shapes", () => {
  const targets = [
    {
      label: "macOS",
      path: "native/macos/Sources/NativeAIHostMac/WebBridge.swift",
      snippets: [
        "hasOnlyRuntimeEnvelopeFields",
        "Runtime bridge envelope contains unknown top-level fields",
        "hasOnlyBridgeRequestFields",
        "Bridge request contains unknown top-level fields",
        "Bridge request id must be a non-empty string",
        "Bridge request timestamp must be a finite number",
        "Bridge request method must be a string",
        "Bridge request params must be an object",
        'body["method"] as! String',
        'body["params"] as! [String: Any]',
      ],
      forbidden: ['body["method"] as? String ?? ""', 'body["params"] as? [String: Any] ?? [:]'],
    },
    {
      label: "iOS",
      path: "native/ios/Sources/NativeAIHostIOS/WebBridge.swift",
      snippets: [
        "hasOnlyRuntimeEnvelopeFields",
        "Runtime bridge envelope contains unknown top-level fields",
        "hasOnlyBridgeRequestFields",
        "Bridge request contains unknown top-level fields",
        "Bridge request id must be a non-empty string",
        "Bridge request timestamp must be a finite number",
        "Bridge request method must be a string",
        "Bridge request params must be an object",
        'body["method"] as! String',
        'body["params"] as! [String: Any]',
      ],
      forbidden: ['body["method"] as? String ?? ""', 'body["params"] as? [String: Any] ?? [:]'],
    },
    {
      label: "Android",
      path: "native/android/app/src/main/java/com/nativeai/platform/NativeBridge.kt",
      snippets: [
        "hasOnlyRuntimeEnvelopeFields",
        "Runtime bridge envelope contains unknown top-level fields",
        "hasOnlyBridgeRequestFields",
        "Bridge request contains unknown top-level fields",
        "Bridge request id must be a non-empty string",
        "Bridge request timestamp must be a finite number",
        "Bridge request method must be a string",
        "Bridge request params must be an object",
        "body.getString(\"method\")",
        "body.getJSONObject(\"params\")",
      ],
      forbidden: ["body.optString(\"method\")", "body.optJSONObject(\"params\") ?: JSONObject()"],
    },
    {
      label: "Windows",
      path: "native/windows/src/WebBridge.cpp",
      snippets: [
        "HasOnlyBridgeRequestFields",
        "Bridge request contains unknown top-level fields",
        "Bridge request timestamp must be a finite number",
        "Bridge request id must be a non-empty string",
        "Bridge request method must be a string",
        "Bridge request params must be an object",
      ],
      forbidden: [],
    },
    {
      label: "Linux",
      path: "native/linux/src/web_bridge.c",
      snippets: [
        "has_only_bridge_request_fields",
        "Bridge request contains unknown top-level fields",
        "Bridge request timestamp must be a finite number",
        "Bridge request id must be a non-empty string",
        "Bridge request method must be a string",
        "Bridge request params must be an object",
      ],
      forbidden: [],
    },
  ];

  for (const target of targets) {
    const source = read(target.path);
    for (const snippet of target.snippets) {
      assert.equal(source.includes(snippet), true, `${target.label} bridge must contain ${snippet}`);
    }
    for (const snippet of target.forbidden) {
      assert.equal(source.includes(snippet), false, `${target.label} bridge must not keep lenient parsing: ${snippet}`);
    }
  }
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
