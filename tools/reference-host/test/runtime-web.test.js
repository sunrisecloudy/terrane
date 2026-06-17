import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { MessageChannel } from "node:worker_threads";

const rootDir = path.resolve(import.meta.dirname, "../../..");
const runtimePath = path.join(rootDir, "runtime-web/runtime.js");
const runtimeExampleAppIds = ["notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab", "calendar-planner"];
const generatedAppCsp = "default-src 'none'; script-src 'self' app-runtime:; style-src 'self' app-runtime:; img-src 'self' app-runtime: data: blob:; font-src 'self' app-runtime:; connect-src 'none'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'; object-src 'none'; require-trusted-types-for 'script'; trusted-types runtime-default;";

test("runtime bridge budget warnings are delivered through AppRuntime.on", async () => {
  const harness = createRuntimeHarness();
  try {
    const frame = await mountFirstApp(harness);
    vm.runInContext(
      'window.AppRuntime.on("app.budget_warning", function (payload) { window.__budgetWarnings.push(payload); });',
      frame.contentWindow.context,
    );

    for (let index = 0; index < 3; index += 1) {
      await vm.runInContext(
        'window.AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] })',
        frame.contentWindow.context,
      );
    }

    await flushAsync();
    assert.deepEqual(frame.contentWindow.__budgetWarnings, [
      {
        appId: "notes-lite",
        budget: "maxBridgeCallsPerMinute",
        current: 4,
        max: 5,
      },
    ]);

    await vm.runInContext(
      'window.AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] })',
      frame.contentWindow.context,
    );
    await flushAsync();
    assert.equal(frame.contentWindow.__budgetWarnings.length, 1);
  } finally {
    harness.close();
  }
});

test("runtime rejects mismatched core.step app before host dispatch", async () => {
  const harness = createRuntimeHarness();
  try {
    const frame = await mountFirstApp(harness);

    await assert.rejects(
      vm.runInContext(
        'window.AppRuntime.call("core.step", { app: "other-app", event: { type: "Probe" } })',
        frame.contentWindow.context,
      ),
      (error) => {
        assert.equal(error.code, "permission_denied");
        assert.equal(error.message, "core.step app field does not match the channel-derived app id");
        assert.deepEqual(error.details, {
          requestedApp: "other-app",
          channelApp: "notes-lite",
        });
        return true;
      },
    );

    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.method === "core.step"), false);
  } finally {
    harness.close();
  }
});

test("runtime rejects appId params before host dispatch", async () => {
  const harness = createRuntimeHarness();
  try {
    const frame = await mountFirstApp(harness);

    await assert.rejects(
      vm.runInContext(
        'window.AppRuntime.call("storage.get", { appId: "other-app", key: "notes-lite:notes", defaultValue: [] })',
        frame.contentWindow.context,
      ),
      (error) => {
        assert.equal(error.code, "invalid_request");
        assert.equal(error.message, "Bridge params must not include appId; app id is channel-derived");
        assert.deepEqual(error.details, { field: "appId" });
        return true;
      },
    );

    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.params.appId === "other-app"), false);
  } finally {
    harness.close();
  }
});

test("runtime rejects private network requests before host dispatch", async () => {
  const harness = createRuntimeHarness({
    manifest: {
      ...defaultRuntimeManifest(),
      permissions: ["network.request"],
      networkPolicy: {
        allow: [
          {
            origin: "https://192.168.0.1",
            methods: ["GET"],
            allowedHeaders: [],
          },
        ],
      },
      resourceBudget: { maxBridgeCallsPerMinute: 5, maxNetworkRequestsPerMinute: 5 },
    },
  });
  try {
    const frame = await mountFirstApp(harness);

    await assert.rejects(
      vm.runInContext(
        [
          'window.AppRuntime.call("network.request", {',
          '  url: "https://192.168.0.1/status",',
          '  method: "GET",',
          '  headers: {},',
          '  body: null',
          "})",
        ].join("\n"),
        frame.contentWindow.context,
      ),
      (error) => {
        assert.equal(error.code, "network_policy_denied");
        assert.equal(error.message, "network.request private network targets are denied");
        assert.deepEqual(error.details, {
          origin: "https://192.168.0.1",
          host: "192.168.0.1",
        });
        return true;
      },
    );

    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.method === "network.request"), false);
  } finally {
    harness.close();
  }
});

test("runtime dev mock handles bridge calls without host dispatch", async () => {
  const harness = createRuntimeHarness({
    devMock: true,
    manifest: {
      ...defaultRuntimeManifest(),
      resourceBudget: { maxBridgeCallsPerMinute: 20 },
    },
  });
  try {
    const frame = await mountFirstApp(harness);

    const setResult = await vm.runInContext(
      'window.AppRuntime.call("storage.set", { key: "notes-lite:notes", value: [{ title: "Mocked" }] })',
      frame.contentWindow.context,
    );
    assert.deepEqual(setResult, { ok: true, bytesWritten: 20 });

    const getResult = await vm.runInContext(
      'window.AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] })',
      frame.contentWindow.context,
    );
    assert.deepEqual(getResult, { value: [{ title: "Mocked" }] });

    const coreResult = await vm.runInContext(
      [
        'window.AppRuntime.call("core.step", {',
        '  app: "notes-lite",',
        '  event: { type: "TransformText", payload: { text: "Hello", mode: "lowercase" } }',
        "})",
      ].join("\n"),
      frame.contentWindow.context,
    );
    assert.deepEqual(coreResult, {
      ok: true,
      stateVersion: 1,
      actions: [{ type: "TransformText", text: "hello" }],
    });

    const createTaskResult = await vm.runInContext(
      [
        'window.AppRuntime.call("core.step", {',
        '  app: "notes-lite",',
        '  event: { type: "CreateTask", payload: { title: "Mock parity" } }',
        "})",
      ].join("\n"),
      frame.contentWindow.context,
    );
    assert.deepEqual(createTaskResult.actions, [
      { type: "Toast", message: "Task accepted: Mock parity", level: "success" },
      { type: "Log", message: "CreateTask handled" },
    ]);

    const networkSnapshotResult = await vm.runInContext(
      [
        'window.AppRuntime.call("core.step", {',
        '  app: "notes-lite",',
        '  event: { type: "NetworkSnapshotReceived", payload: { status: 200 } }',
        "})",
      ].join("\n"),
      frame.contentWindow.context,
    );
    assert.deepEqual(networkSnapshotResult.actions, [
      { type: "RenderHint", hint: "network-snapshot-received" },
    ]);

    const unknownResult = await vm.runInContext(
      [
        'window.AppRuntime.call("core.step", {',
        '  app: "notes-lite",',
        '  event: { type: "ProbeEvent", payload: {} }',
        "})",
      ].join("\n"),
      frame.contentWindow.context,
    );
    assert.deepEqual(unknownResult.actions, [
      { type: "Log", message: "Unhandled event: ProbeEvent" },
    ]);

    assert.equal(harness.fetchState.bridgeRequests.length, 0);
  } finally {
    harness.close();
  }
});

test("runtime exposes development-only devtools hooks", async () => {
  const harness = createRuntimeHarness({
    devMock: true,
    manifest: {
      ...defaultRuntimeManifest(),
      permissions: ["core.step", "storage.read", "storage.write", "app.log"],
    },
  });
  try {
    const frame = await mountFirstApp(harness);
    const devtools = harness.parentWindow.__APP_RUNTIME_DEVTOOLS__;
    assert.equal(typeof devtools.snapshot, "function");
    assert.equal(typeof devtools.query, "function");
    assert.equal(typeof devtools.bridgeLog, "function");
    assert.equal(typeof devtools.consoleLog, "function");
    assert.equal(typeof devtools.storageSnapshot, "function");
    assert.equal(typeof devtools.coreEventLog, "function");
    assert.equal(typeof devtools.reset, "function");

    await vm.runInContext(
      'window.AppRuntime.call("storage.set", { key: "notes-lite:devtools", value: { title: "Debuggable" } })',
      frame.contentWindow.context,
    );
    await vm.runInContext(
      [
        'window.AppRuntime.call("core.step", {',
        '  app: "notes-lite",',
        '  event: { type: "TransformText", payload: { text: "Hi", mode: "lowercase" } }',
        "})",
      ].join("\n"),
      frame.contentWindow.context,
    );
    await vm.runInContext(
      'window.AppRuntime.call("app.log", { level: "info", message: "devtools ready" })',
      frame.contentWindow.context,
    );

    const snapshot = devtools.snapshot();
    assert.equal(snapshot.activeApp.appId, "notes-lite");
    assert.equal(snapshot.mounted, true);
    assert.equal(snapshot.testIds.includes("app-root"), true);
    assert.equal(devtools.query({ testId: "app-root" }).matches[0].tagName, "main");
    assert.equal(devtools.query('[data-testid="app-root"]').count, 1);
    assert.equal(devtools.bridgeLog().some((line) => /notes-lite storage\.set ok/.test(line)), true);
    assert.deepEqual(JSON.parse(JSON.stringify(devtools.storageSnapshot("notes-lite").entries)), [
      { key: "notes-lite:devtools", value: { title: "Debuggable" } },
    ]);
    assert.equal(devtools.coreEventLog("notes-lite").length, 1);
    assert.equal(devtools.consoleLog().some((line) => line.message === "devtools ready"), true);

    assert.deepEqual(JSON.parse(JSON.stringify(devtools.reset("notes-lite"))), { ok: true, appId: "notes-lite" });
    assert.deepEqual(JSON.parse(JSON.stringify(devtools.storageSnapshot("notes-lite").entries)), []);
    assert.deepEqual(JSON.parse(JSON.stringify(devtools.coreEventLog("notes-lite"))), []);
    assert.deepEqual(JSON.parse(JSON.stringify(devtools.consoleLog())), []);
  } finally {
    harness.close();
  }
});

test("runtime exposes devtools hooks when the host enables them", async () => {
  const harness = createRuntimeHarness({ devtools: true });
  try {
    await loadRuntime(harness);
    assert.equal(typeof harness.parentWindow.__APP_RUNTIME_DEVTOOLS__.snapshot, "function");
  } finally {
    harness.close();
  }
});

test("runtime hides devtools hooks outside dev/test mode", async () => {
  const harness = createRuntimeHarness();
  try {
    await loadRuntime(harness);
    assert.equal(harness.parentWindow.__APP_RUNTIME_DEVTOOLS__, undefined);
  } finally {
    harness.close();
  }
});

test("runtime launcher, sandbox, bridge calls, debug log, and structured errors work", async () => {
  const manifestsById = new Map(
    runtimeExampleAppIds.map((appId) => [
      appId,
      {
        ...defaultRuntimeManifest(),
        id: appId,
        name: appId,
        description: `${appId} acceptance fixture`,
        storagePrefix: `${appId}:`,
        permissions: ["core.step", "storage.read", "storage.write"],
        resourceBudget: { maxBridgeCallsPerMinute: 20 },
      },
    ]),
  );
  const harness = createRuntimeHarness({ manifestsById });
  try {
    const frame = await mountFirstApp(harness);
    assert.equal(harness.document.getElementById("app-list").children.length, runtimeExampleAppIds.length);
    assert.equal(frame.attributes.get("sandbox"), "allow-scripts");
    assert.equal(frame.attributes.get("allow"), "");
    assert.equal(frame.attributes.get("csp"), generatedAppCsp);
    assert.equal(frame.srcdoc.includes("<base "), false);
    assert.equal(typeof frame.contentWindow.AppRuntime.call, "function");

    vm.runInContext(
      'window.__appErrors = []; window.AppRuntime.on("app.error", function (payload) { window.__appErrors.push(payload); });',
      frame.contentWindow.context,
    );

    const storageResult = await vm.runInContext(
      'window.AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] })',
      frame.contentWindow.context,
    );
    assert.deepEqual(storageResult, {});

    await assert.rejects(
      vm.runInContext('window.AppRuntime.call("runtime.not_real", {})', frame.contentWindow.context),
      (error) => {
        assert.equal(error.code, "unknown_method");
        assert.equal(error.message, "Unknown bridge method: runtime.not_real");
        assert.deepEqual(error.details, { method: "runtime.not_real" });
        return true;
      },
    );

    await assert.rejects(
      vm.runInContext(
        'window.AppRuntime.call("notification.toast", { message: "Denied" })',
        frame.contentWindow.context,
      ),
      (error) => {
        assert.equal(error.code, "permission_denied");
        assert.equal(error.details.requiredPermission, "notification.toast");
        return true;
      },
    );

    await assert.rejects(
      vm.runInContext(
        'window.AppRuntime.call("storage.get", { key: "task-workbench:tasks", defaultValue: [] })',
        frame.contentWindow.context,
      ),
      (error) => {
        assert.equal(error.code, "permission_denied");
        assert.equal(error.details.prefix, "notes-lite:");
        return true;
      },
    );

    await flushAsync();
    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.method === "storage.get"), true);
    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.method === "runtime.not_real"), false);
    assert.equal(harness.fetchState.bridgeRequests.some((request) => request.method === "notification.toast"), false);
    const bridgeLogText = harness.document
      .getElementById("bridge-log")
      .children.map((child) => child.textContent)
      .join("\n");
    assert.match(bridgeLogText, /notes-lite storage\.get ok/);
    assert.match(bridgeLogText, /notes-lite runtime\.not_real unknown_method/);
    assert.match(bridgeLogText, /notes-lite notification\.toast permission_denied/);
    assert.match(bridgeLogText, /notes-lite storage\.get permission_denied/);
    assert.deepEqual(
      Array.from(frame.contentWindow.__appErrors, (error) => error.code),
      ["unknown_method", "permission_denied", "permission_denied"],
    );
  } finally {
    harness.close();
  }
});

test("runtime uses host app index with content ratings when available", async () => {
  const manifestsById = new Map([
    [
      "notes-lite",
      {
        ...defaultRuntimeManifest(),
        id: "notes-lite",
        name: "Notes Lite",
        description: "Allowed by the host content rating gate.",
        storagePrefix: "notes-lite:",
        permissions: ["storage.read"],
      },
    ],
    [
      "api-dashboard",
      {
        ...defaultRuntimeManifest(),
        id: "api-dashboard",
        name: "API Dashboard",
        description: "Filtered out by the host app index.",
        storagePrefix: "api-dashboard:",
        permissions: ["storage.read"],
      },
    ],
  ]);
  const appIndex = {
    source: "ios-bundled",
    apps: [
      {
        id: "notes-lite",
        name: "Notes Lite",
        description: "Allowed by the host content rating gate.",
        version: "0.1.0",
        contentRating: { scheme: "app-store", label: "4+", minimumAge: 4, descriptors: [] },
      },
    ],
  };
  const harness = createRuntimeHarness({ appIndex, manifestsById });
  try {
    await loadRuntime(harness);
    assert.equal(harness.fetchState.appIndexRequests, 1);
    assert.equal(harness.document.getElementById("app-list").children.length, 1);
    const appButton = harness.document.getElementById("app-list").children[0];
    assert.equal(appButton.querySelector("strong").textContent, "Notes Lite");
    assert.match(appButton.querySelector("span").textContent, /notes-lite v0\.1\.0 · 4\+/);

    const frame = await mountAppAtIndex(harness, 0);
    assert.equal(frame.title, "Notes Lite");
    assert.equal(frame.srcdoc.includes("<base "), false);
    assert.match(frame.srcdoc, /<link rel="stylesheet" href="\/webapps\/examples\/notes-lite\/styles\.css">/);
    assert.match(frame.srcdoc, /<script src="\/webapps\/examples\/notes-lite\/app\.js"><\/script>/);
  } finally {
    harness.close();
  }
});

test("runtime exposes native host mode for AppKit sidebar mounting", async () => {
  const manifestsById = new Map([
    [
      "notes-lite",
      {
        ...defaultRuntimeManifest(),
        id: "notes-lite",
        name: "Notes Lite",
        description: "Storage fixture.",
        storagePrefix: "notes-lite:",
      },
    ],
    [
      "task-workbench",
      {
        ...defaultRuntimeManifest(),
        id: "task-workbench",
        name: "Task Workbench",
        description: "Native sidebar target.",
        storagePrefix: "task-workbench:",
      },
    ],
  ]);
  const appIndex = {
    source: "macos-bundled",
    apps: [
      { id: "notes-lite", name: "Notes Lite", version: "0.1.0", description: "Storage fixture." },
      { id: "task-workbench", name: "Task Workbench", version: "0.1.0", description: "Native sidebar target." },
    ],
  };
  const harness = createRuntimeHarness({ appIndex, manifestsById });
  try {
    await loadRuntime(harness);
    assert.equal(typeof harness.parentWindow.TerraneRuntimeHost.mountApp, "function");

    const result = await vm.runInContext(
      'window.TerraneRuntimeHost.setHostMode(true); window.TerraneRuntimeHost.mountApp("task-workbench")',
      harness.parentContext,
    );
    await flushAsync();

    assert.equal(result.ok, true);
    assert.equal(result.appId, "task-workbench");
    assert.equal(harness.document.body.classList.contains("native-host-mode"), true);
    assert.equal(harness.parentWindow.TerraneRuntimeHost.activeAppId(), "task-workbench");
    const frame = harness.document.getElementById("app-frame-wrap").children[0];
    assert.ok(frame, "native host mount should create an app iframe");
    assert.equal(frame.title, "Task Workbench");
    assert.match(frame.srcdoc, /\/webapps\/examples\/task-workbench\/app\.js/);
  } finally {
    harness.close();
  }
});

test("runtime native host mount rejects unknown app ids", async () => {
  const manifestsById = new Map([
    [
      "notes-lite",
      {
        ...defaultRuntimeManifest(),
        id: "notes-lite",
        name: "Notes Lite",
        description: "Storage fixture.",
        storagePrefix: "notes-lite:",
      },
    ],
  ]);
  const appIndex = {
    source: "macos-bundled",
    apps: [
      { id: "notes-lite", name: "Notes Lite", version: "0.1.0", description: "Storage fixture." },
    ],
  };
  const harness = createRuntimeHarness({ appIndex, manifestsById });
  try {
    await loadRuntime(harness);
    await assert.rejects(
      vm.runInContext('window.TerraneRuntimeHost.mountApp("missing-app")', harness.parentContext),
      /Unknown Terrane app: missing-app/,
    );
    assert.equal(harness.parentWindow.TerraneRuntimeHost.activeAppId(), null);
  } finally {
    harness.close();
  }
});

test("runtime can mount every bundled app in a sandboxed frame", async () => {
  const manifestsById = new Map(
    runtimeExampleAppIds.map((appId) => [
      appId,
      {
        ...defaultRuntimeManifest(),
        id: appId,
        name: appId,
        description: `${appId} sandbox fixture`,
        storagePrefix: `${appId}:`,
        permissions: ["storage.read"],
        resourceBudget: { maxBridgeCallsPerMinute: 20 },
      },
    ]),
  );
  const harness = createRuntimeHarness({ manifestsById });
  try {
    await loadRuntime(harness);
    assert.equal(harness.document.getElementById("app-list").children.length, runtimeExampleAppIds.length);

    for (const [index, appId] of runtimeExampleAppIds.entries()) {
      const frame = await mountAppAtIndex(harness, index);
      assert.equal(harness.document.getElementById("active-title").textContent, appId);
      assert.equal(frame.title, appId);
      assert.equal(frame.attributes.get("sandbox"), "allow-scripts");
      assert.equal(frame.attributes.get("csp"), generatedAppCsp);
      assert.equal(frame.srcdoc.includes("<base "), false);
      assert.match(frame.srcdoc, new RegExp(`<link rel="stylesheet" href="/webapps/examples/${appId}/styles\\.css">`));
      assert.match(frame.srcdoc, new RegExp(`<script src="/webapps/examples/${appId}/app\\.js"></script>`));
      assert.equal(typeof frame.contentWindow.AppRuntime.call, "function");

      await vm.runInContext(
        `window.AppRuntime.call("storage.get", { key: "${appId}:probe", defaultValue: [] })`,
        frame.contentWindow.context,
      );
      await flushAsync();

      const request = harness.fetchState.bridgeRequests.at(-1);
      assert.equal(request.method, "storage.get");
      assert.equal(request.params.key, `${appId}:probe`);
      assert.equal(request.headers["x-app-id"], appId);
    }
  } finally {
    harness.close();
  }
});

test("runtime keeps WebKit native app frames on the parent-owned sandbox channel", async () => {
  const harness = createRuntimeHarness();
  harness.parentWindow.location = { protocol: "app-runtime:", hostname: "runtime", search: "" };
  harness.parentWindow.webkit = {
    messageHandlers: {
      TerranePlatformBridge: {
        postMessage() {
          return Promise.resolve({ ok: true, result: { runtimeVersion: "test" } });
        },
      },
    },
  };
  try {
    await loadRuntime(harness);

    const appButton = harness.document.getElementById("app-list").children[0];
    appButton.dispatch("click");
    await flushAsync();

    const frame = harness.document.getElementById("app-frame-wrap").children[0];
    assert.ok(frame, "runtime launcher should mount the selected app iframe");
    assert.equal(frame.attributes.get("sandbox"), "allow-scripts");
    assert.equal(frame.attributes.get("allow"), "");
    assert.equal(frame.attributes.get("csp"), generatedAppCsp);
    assert.match(frame.src, /^app-runtime:\/\/notes-lite\/index\.html\?mountToken=/);
    assert.equal(frame.srcdoc, undefined);
  } finally {
    harness.close();
  }
});

test("runtime sends WebKit native bridge envelopes from the parent-owned port mount", async () => {
  const nativeEnvelopes = [];
  let appPort;
  const harness = createRuntimeHarness();
  harness.parentWindow.location = { protocol: "app-runtime:", hostname: "runtime", search: "" };
  harness.parentWindow.webkit = {
    messageHandlers: {
      TerranePlatformBridge: {
        postMessage(envelope) {
          nativeEnvelopes.push(envelope);
          if (envelope.request.method === "runtime.capabilities") {
            return Promise.resolve({ id: envelope.request.id, ok: true, result: { runtimeVersion: "test", features: {} } });
          }
          return Promise.resolve({ id: envelope.request.id, ok: true, result: { value: ["native"] } });
        },
      },
    },
  };
  try {
    await loadRuntime(harness);

    const appButton = harness.document.getElementById("app-list").children[0];
    appButton.dispatch("click");
    await flushAsync();

    const frame = harness.document.getElementById("app-frame-wrap").children[0];
    frame.contentWindow.addEventListener("message", (event) => {
      if (event.data && event.data.type === "runtime.port") {
        appPort = event.ports[0];
      }
    });
    frame.contentWindow.parent.postMessage({ type: "runtime.ready_for_port" }, "*");
    await flushAsync();

    assert.ok(appPort, "WebKit app frame should receive a MessagePort from the parent runtime");
    const result = await postPortRequest(appPort, {
      id: "webkit_storage_probe",
      method: "storage.get",
      params: { key: "notes-lite:notes", defaultValue: [] },
      timestamp: Date.now(),
    });
    await flushAsync();

    assert.deepEqual(result.result, { value: ["native"] });
    const storageEnvelope = nativeEnvelopes.find((envelope) => envelope.request.method === "storage.get");
    assert.ok(storageEnvelope, "storage.get should dispatch through the native WebKit handler");
    assert.equal(storageEnvelope.appId, "notes-lite");
    assert.equal(typeof storageEnvelope.mountToken, "string");
    assert.equal(storageEnvelope.request.params.key, "notes-lite:notes");
    assert.equal("appId" in storageEnvelope.request.params, false);
    assert.equal(harness.fetchState.bridgeRequests.length, 0);
  } finally {
    if (appPort) appPort.close();
    harness.close();
  }
});

test("runtime uses WebView2 host-issued mount tokens for native bridge envelopes", async () => {
  const nativeMessages = [];
  const webview2Listeners = [];
  const harness = createRuntimeHarness();
  harness.parentWindow.chrome = {
    webview: {
      addEventListener(type, handler) {
        if (type === "message") webview2Listeners.push(handler);
      },
      postMessage(message) {
        const parsed = JSON.parse(message);
        nativeMessages.push(parsed);
        if (parsed.type === "runtime.mount_request") {
          setImmediate(() => {
            for (const handler of webview2Listeners) {
              handler({
                data: JSON.stringify({
                  type: "runtime.mount_response",
                  id: parsed.id,
                  ok: true,
                  appId: parsed.appId,
                  mountToken: "native-issued-webview2-token",
                }),
              });
            }
          });
          return;
        }
        if (parsed.request) {
          setImmediate(() => {
            for (const handler of webview2Listeners) {
              handler({
                data: JSON.stringify({
                  id: parsed.request.id,
                  ok: true,
                  result: parsed.request.method === "runtime.capabilities" ? { runtimeVersion: "test", features: {} } : { value: ["webview2"] },
                }),
              });
            }
          });
        }
      },
    },
  };

  try {
    await loadRuntime(harness);
    const frame = await mountAppAtIndex(harness, 0);
    const result = await vm.runInContext(
      'window.AppRuntime.call("storage.get", { key: "notes-lite:notes", defaultValue: [] })',
      frame.contentWindow.context,
    );
    await flushAsync();

    const mountRequest = nativeMessages.find((message) => message.type === "runtime.mount_request");
    assert.ok(mountRequest, "runtime should request a WebView2 host-issued mount token");
    assert.equal(mountRequest.appId, "notes-lite");
    const storageEnvelope = nativeMessages.find((message) => message.request && message.request.method === "storage.get");
    assert.ok(storageEnvelope, "storage.get should dispatch through the native WebView2 handler");
    assert.equal(storageEnvelope.appId, "notes-lite");
    assert.equal(storageEnvelope.mountToken, "native-issued-webview2-token");
    assert.deepEqual(result, { value: ["webview2"] });
    assert.equal(harness.fetchState.bridgeRequests.length, 0);
  } finally {
    harness.close();
  }
});

test("runtime emits app.error when a sandbox posts a bridge request outside its assigned channel", async () => {
  const harness = createRuntimeHarness();
  try {
    const frame = await mountFirstApp(harness);
    vm.runInContext(
      'window.__appErrors = []; window.AppRuntime.on("app.error", function (payload) { window.__appErrors.push(payload); });',
      frame.contentWindow.context,
    );
    vm.runInContext(
      'window.parent.postMessage({ id: "direct-parent", method: "storage.get", params: { key: "notes-lite:notes" } }, "*");',
      frame.contentWindow.context,
    );
    await flushAsync();

    const errors = vm.runInContext("window.__appErrors", frame.contentWindow.context);
    assert.equal(errors.length, 1);
    assert.equal(errors[0].code, "bridge.unauthorized_channel");
    assert.equal(errors[0].source, "postMessage");
  } finally {
    harness.close();
  }
});

async function loadRuntime(harness) {
  const runtimeSource = fs.readFileSync(runtimePath, "utf8");
  vm.runInContext(runtimeSource, harness.parentContext);

  await flushAsync();
  assert.ok(harness.document.getElementById("app-list").children[0], "runtime launcher should render app buttons");
}

async function mountAppAtIndex(harness, index) {
  const appButton = harness.document.getElementById("app-list").children[index];
  assert.ok(appButton, `runtime launcher should render app button ${index}`);
  appButton.dispatch("click");
  await flushAsync();

  const frame = harness.document.getElementById("app-frame-wrap").children[0];
  assert.ok(frame, "runtime launcher should mount the selected app iframe");
  const bootstrapSource = extractBootstrapScript(frame.srcdoc);
  frame.contentWindow.__budgetWarnings = [];
  vm.runInContext(bootstrapSource, frame.contentWindow.context);

  await flushAsync();
  return frame;
}

async function mountFirstApp(harness) {
  await loadRuntime(harness);
  return mountAppAtIndex(harness, 0);
}

function createRuntimeHarness(options = {}) {
  let parentWindow;
  const messageChannels = [];
  const runtimeConsole = options.console ?? {
    debug() {},
    error() {},
    info() {},
    log() {},
    warn() {},
  };
  function HarnessMessageChannel() {
    const channel = new MessageChannel();
    messageChannels.push(channel);
    return channel;
  }
  const document = createDocument(() => createChildWindow(parentWindow));
  parentWindow = createWindow();
  parentWindow.document = document;
  parentWindow.crypto = webcrypto;
  parentWindow.__APP_RUNTIME_DEV_MOCK__ = options.devMock === true;
  parentWindow.__APP_RUNTIME_DEVTOOLS_ENABLED__ = options.devtools === true;
  parentWindow.location = options.location ?? { hostname: "runtime.local.platform", search: "" };
  parentWindow.dispatchMessage = function (event) {
    parentWindow.dispatch("message", event);
  };

  const fetchState = {
    appIndex: options.appIndex,
    appIndexRequests: 0,
    bridgeRequests: [],
    manifest: options.manifest ?? defaultRuntimeManifest(),
    manifestsById: options.manifestsById ?? null,
  };
  const parentContext = vm.createContext({
    MessageChannel: HarnessMessageChannel,
    TextEncoder,
    URL,
    btoa: (value) => Buffer.from(value, "binary").toString("base64"),
    clearInterval,
    clearTimeout,
    console: runtimeConsole,
    crypto: webcrypto,
    document,
    fetch: (url, options) => fakeFetch(url, options, fetchState),
    setInterval,
    setTimeout,
    window: parentWindow,
  });
  parentWindow.context = parentContext;
  return {
    close() {
      for (const channel of messageChannels) {
        channel.port1.close();
        channel.port2.close();
      }
    },
    document,
    fetchState,
    parentContext,
    parentWindow,
  };
}

function createChildWindow(parentWindow) {
  const childWindow = createWindow();
  const document = createChildDocument();
  childWindow.document = document;
  childWindow.parent = {
    postMessage(data, _targetOrigin, ports = []) {
      parentWindow.dispatchMessage({
        data,
        ports,
        source: childWindow,
      });
    },
  };
  childWindow.postMessage = function (data, _targetOrigin, ports = []) {
    childWindow.dispatch("message", {
      data,
      ports,
      source: parentWindow,
    });
  };
  childWindow.crypto = webcrypto;
  childWindow.setTimeout = setTimeout;
  childWindow.clearTimeout = clearTimeout;
  childWindow.setInterval = setInterval;
  childWindow.clearInterval = clearInterval;
  const context = vm.createContext({
    clearInterval,
    clearTimeout,
    console,
    document,
    setInterval,
    setTimeout,
    window: childWindow,
  });
  childWindow.context = context;
  return childWindow;
}

function createWindow() {
  const listeners = new Map();
  return {
    addEventListener(type, handler) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(handler);
    },
    dispatch(type, event) {
      for (const handler of listeners.get(type) || []) {
        handler(event);
      }
    },
  };
}

function createDocument(createFrameWindow) {
  const elements = new Map();
  const body = new FakeElement("body");
  for (const id of [
    "active-description",
    "active-title",
    "app-frame-wrap",
    "app-list",
    "bridge-log",
    "clear-debug",
    "refresh-apps",
    "reload-app",
    "runtime-status",
  ]) {
    elements.set(id, new FakeElement(id === "reload-app" || id === "refresh-apps" || id === "clear-debug" ? "button" : "div"));
  }
  return {
    body,
    createElement(tagName) {
      const element = new FakeElement(tagName);
      if (tagName === "iframe") {
        element.contentWindow = createFrameWindow();
      }
      return element;
    },
    getElementById(id) {
      return elements.get(id) || null;
    },
  };
}

function createChildDocument() {
  return {
    documentElement: {},
    getElementsByTagName() {
      return [];
    },
  };
}

class FakeElement {
  constructor(tagName) {
    this.attributes = new Map();
    this.children = [];
    this.className = "";
    this.contentWindow = null;
    this.dataset = {};
    this.disabled = false;
    this.listeners = new Map();
    this.queryChildren = new Map();
    this.tagName = tagName.toUpperCase();
    this._textContent = "";
  }

  set innerHTML(value) {
    this.html = value;
    this.queryChildren.clear();
    if (value.includes("<strong")) this.queryChildren.set("strong", new FakeElement("strong"));
    if (value.includes("<span")) this.queryChildren.set("span", new FakeElement("span"));
  }

  get innerHTML() {
    return this.html || "";
  }

  set textContent(value) {
    this._textContent = String(value);
    this.children = [];
  }

  get textContent() {
    return this._textContent;
  }

  get classList() {
    return {
      add: (name) => {
        if (!this.className.split(/\s+/).includes(name)) {
          this.className = `${this.className} ${name}`.trim();
        }
      },
      contains: (name) => this.className.split(/\s+/).includes(name),
      remove: (name) => {
        this.className = this.className.split(/\s+/).filter((part) => part && part !== name).join(" ");
      },
      toggle: (name, force) => {
        const shouldAdd = force === undefined ? !this.className.split(/\s+/).includes(name) : Boolean(force);
        if (shouldAdd) {
          if (!this.className.split(/\s+/).includes(name)) {
            this.className = `${this.className} ${name}`.trim();
          }
        } else {
          this.className = this.className.split(/\s+/).filter((part) => part && part !== name).join(" ");
        }
        return shouldAdd;
      },
    };
  }

  addEventListener(type, handler) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(handler);
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  dispatch(type) {
    for (const handler of this.listeners.get(type) || []) {
      handler({ target: this });
    }
  }

  prepend(child) {
    this.children.unshift(child);
    return child;
  }

  querySelector(selector) {
    return this.queryChildren.get(selector) || null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, value);
  }
}

async function fakeFetch(url, options = {}, state = { bridgeRequests: [] }) {
  if (url === "/runtime/app-index.json") {
    state.appIndexRequests = (state.appIndexRequests || 0) + 1;
    if (state.appIndex) return jsonResponse(state.appIndex);
    return notFoundResponse();
  }
  const manifestMatch = url.match(/^\/webapps\/examples\/([^/]+)\/manifest\.json$/);
  if (manifestMatch) {
    const manifest = state.manifestsById?.get(manifestMatch[1]) ?? state.manifest ?? defaultRuntimeManifest();
    return jsonResponse(manifest);
  }
  if (url.endsWith("/manifest.json")) {
    return jsonResponse(state.manifest ?? defaultRuntimeManifest());
  }
  if (url.endsWith("/index.html")) {
    return textResponse("<!doctype html><html><head><link rel=\"stylesheet\" href=\"styles.css\"></head><body><main data-testid=\"app-root\"></main><script src=\"app.js\"></script></body></html>");
  }
  if (url === "/bridge") {
    const request = JSON.parse(options.body);
    state.bridgeRequests.push({ ...request, headers: options.headers ?? {} });
    return jsonResponse({
      id: request.id,
      ok: true,
      result: request.method === "runtime.capabilities" ? { runtimeVersion: "test" } : {},
    });
  }
  throw new Error(`Unexpected fetch URL in runtime harness: ${url}`);
}

function defaultRuntimeManifest() {
  return {
    id: "notes-lite",
    name: "Notes Lite",
    version: "0.1.0",
    description: "Budget warning probe app.",
    contentRating: { scheme: "app-store", label: "4+", minimumAge: 4, descriptors: [] },
    permissions: ["core.step", "storage.read", "storage.write"],
    storagePrefix: "notes-lite:",
    capabilities: [],
    dataVersion: "1.0.0",
    networkPolicy: { allow: [] },
    resourceBudget: { maxBridgeCallsPerMinute: 5 },
  };
}

function notFoundResponse() {
  return {
    ok: false,
    status: 404,
    json: async () => ({ error: "not_found" }),
    text: async () => "not found",
  };
}

function jsonResponse(body) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

function textResponse(body) {
  return {
    ok: true,
    status: 200,
    json: async () => JSON.parse(body),
    text: async () => body,
  };
}

function extractBootstrapScript(srcdoc) {
  const match = srcdoc.match(/<script>\n?([\s\S]*?)<\/script>/);
  assert.ok(match, "mounted app srcdoc should include the runtime bootstrap script");
  return match[1];
}

async function postPortRequest(port, request) {
  return new Promise((resolve) => {
    port.once("message", resolve);
    port.postMessage(request);
  });
}

async function flushAsync() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}
