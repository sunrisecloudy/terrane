import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { BridgeDispatcher } from "../src/bridge-dispatcher.js";
import { CoreEngine } from "../src/core.js";
import { referenceHostCapabilities } from "../src/capabilities.js";
import { repoRoot } from "../src/paths.js";
import { PlatformDatabase } from "../src/platform-database.js";

const appId = "notes-lite";
const capabilityFixturesDir = path.join(repoRoot, "tests", "fixtures", "capabilities");
const expectedFixtureFiles = [
  "android.json",
  "ios-simulator.json",
  "linux.json",
  "macos.json",
  "reference-host.json",
  "server.json",
  "windows.json",
];
const requiredFeatureIds = [
  "runtime.capabilities",
  "storage.read",
  "storage.write",
  "storage.get",
  "storage.set",
  "storage.remove",
  "storage.list",
  "dialog.openFile",
  "dialog.saveFile",
  "notification.toast",
  "network.request",
  "core.step",
  "app.log",
  "notebook.read",
  "notebook.write",
  "notebook.propose",
  "notebook.approve",
  "notebook.sync",
];

test("runtime capabilities schema allows channel-derived appId", () => {
  const schema = readJson(path.join(repoRoot, "schemas", "runtime-capabilities.schema.json"));
  assert.equal(schema.additionalProperties, false);
  assert.equal(schema.properties?.appId?.type, "string");
  assert.equal(schema.required.includes("appId"), false);
});

test("checked-in runtime capability fixtures are schema-shaped for every target", () => {
  const files = fs.readdirSync(capabilityFixturesDir).filter((fileName) => fileName.endsWith(".json")).sort();
  assert.deepEqual(files, expectedFixtureFiles);

  for (const fileName of files) {
    const fixture = readJson(path.join(capabilityFixturesDir, fileName));
    assertRuntimeCapabilitiesShape(fixture, `${fileName} fixture`);
    assert.equal(fixture.appId, appId, `${fileName} fixture exposes channel-derived app id`);
    for (const feature of requiredFeatureIds) {
      assert.equal(typeof fixture.features[feature], "boolean", `${fileName} fixture feature ${feature}`);
    }
  }
});

test("reference-host runtime.capabilities bridge response is app-scoped and schema-shaped", async () => {
  const db = new PlatformDatabase();
  try {
    const dispatcher = new BridgeDispatcher({ database: db, core: new CoreEngine() });
    const sessionId = db.createRuntimeSession({ appId });
    const response = await dispatcher.dispatch(
      { id: "req_caps", method: "runtime.capabilities", params: {} },
      { appId, sessionId },
    );

    assert.equal(response.ok, true);
    assertRuntimeCapabilitiesShape(response.result, "reference-host bridge response");
    assert.equal(response.result.appId, appId);
    assert.deepEqual(response.result, referenceHostCapabilities({ appId }));
  } finally {
    db.close();
  }
});

test("native and server capability implementations expose app-scoped manifest capability ids", () => {
  const contracts = [
    {
      target: "macos",
      source: "native/macos/Sources/TerraneHostMac/WebBridge.swift",
      snippets: [
        '"platform": "macos"',
        '"target": "macos"',
        '"appId": request.context.appId',
        '"devMode": nativeDevMode',
        '"storage.read": true',
        '"storage.write": true',
        '"runtime.capabilities": true',
        '"core.step": core.isAvailable',
      ],
    },
    {
      target: "ios-simulator",
      source: "native/ios/Sources/TerraneHostIOS/WebBridge.swift",
      snippets: [
        '"platform": "ios"',
        '"target": "ios-simulator"',
        '"appId": request.context.appId',
        '"devMode": nativeDevMode',
        '"storage.read": true',
        '"storage.write": true',
        '"runtime.capabilities": true',
        '"core.step": core.isAvailable',
      ],
    },
    {
      target: "android",
      source: "native/android/app/src/main/java/com/terrane/platform/NativeBridge.kt",
      snippets: [
        '"platform" to "android"',
        '"target" to "android"',
        '"appId" to request.context.appId',
        '"devMode" to BuildConfig.DEBUG',
        '"storage.read" to true',
        '"storage.write" to true',
        '"runtime.capabilities" to true',
        '"core.step" to core.isAvailable()',
      ],
    },
    {
      target: "windows",
      source: "native/windows/src/WebBridge.cpp",
      snippets: [
        'result.Insert(L"platform", json::JsonValue::CreateStringValue(L"windows"))',
        'result.Insert(L"target", json::JsonValue::CreateStringValue(L"windows"))',
        'result.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId))',
        'result.Insert(L"devMode", json::JsonValue::CreateBooleanValue(NativeDevMode()))',
        'features.Insert(L"storage.read", json::JsonValue::CreateBooleanValue(true))',
        'features.Insert(L"storage.write", json::JsonValue::CreateBooleanValue(true))',
        'features.Insert(L"dialog.openFile", json::JsonValue::CreateBooleanValue(true))',
        'features.Insert(L"dialog.saveFile", json::JsonValue::CreateBooleanValue(true))',
        'features.Insert(L"runtime.capabilities", json::JsonValue::CreateBooleanValue(true))',
        'features.Insert(L"core.step", json::JsonValue::CreateBooleanValue(core_.IsAvailable()))',
      ],
    },
    {
      target: "linux",
      source: "native/linux/src/web_bridge.c",
      snippets: [
        'json_builder_add_string_value(builder, "linux")',
        'json_builder_set_member_name(builder, "appId")',
        "request->context.app_id",
        "native_dev_mode()",
        '"storage.read"',
        '"storage.write"',
        '"runtime.capabilities"',
        "zig_core_bridge_is_available(&bridge->core)",
      ],
    },
    {
      target: "server",
      source: "server/src/main.zig",
      snippets: [
        '\\"platform\\":\\"server\\"',
        '\\"target\\":\\"zig-server\\"',
        ',\\"appId\\":\\"{s}\\"',
        '\\"storage.read\\":true',
        '\\"storage.write\\":true',
        '\\"storage.get\\":true',
        '\\"storage.set\\":true',
        '\\"runtime.capabilities\\":true',
      ],
    },
  ];

  for (const contract of contracts) {
    const source = fs.readFileSync(path.join(repoRoot, contract.source), "utf8");
    for (const snippet of contract.snippets) {
      assert.equal(source.includes(snippet), true, `${contract.target} runtime.capabilities missing ${snippet}`);
    }
  }
});

function assertRuntimeCapabilitiesShape(value, label) {
  assert.equal(isRecord(value), true, `${label} must be an object`);
  for (const key of ["runtimeVersion", "platform", "target"]) {
    assert.equal(typeof value[key], "string", `${label}.${key}`);
    assert.notEqual(value[key].length, 0, `${label}.${key} must not be empty`);
  }
  assert.equal(typeof value.appId, "string", `${label}.appId`);
  assert.notEqual(value.appId.length, 0, `${label}.appId must not be empty`);
  assert.equal(typeof value.devMode, "boolean", `${label}.devMode`);
  assert.equal(isRecord(value.features), true, `${label}.features`);
  assert.equal(isRecord(value.limits), true, `${label}.limits`);
  for (const [feature, enabled] of Object.entries(value.features)) {
    assert.equal(typeof enabled, "boolean", `${label}.features.${feature}`);
  }
  for (const [limit, amount] of Object.entries(value.limits)) {
    assert.equal(Number.isInteger(amount), true, `${label}.limits.${limit}`);
    assert.equal(amount >= 0, true, `${label}.limits.${limit}`);
  }
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}
