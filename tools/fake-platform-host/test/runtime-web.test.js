import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { MessageChannel } from "node:worker_threads";

const rootDir = path.resolve(import.meta.dirname, "../../..");
const runtimePath = path.join(rootDir, "runtime-web/runtime.js");

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

async function mountFirstApp(harness) {
  const runtimeSource = fs.readFileSync(runtimePath, "utf8");
  vm.runInContext(runtimeSource, harness.parentContext);

  await flushAsync();
  const firstAppButton = harness.document.getElementById("app-list").children[0];
  assert.ok(firstAppButton, "runtime launcher should render app buttons");
  firstAppButton.dispatch("click");
  await flushAsync();

  const frame = harness.document.getElementById("app-frame-wrap").children[0];
  assert.ok(frame, "runtime launcher should mount the selected app iframe");
  const bootstrapSource = extractBootstrapScript(frame.srcdoc);
  frame.contentWindow.__budgetWarnings = [];
  vm.runInContext(bootstrapSource, frame.contentWindow.context);

  await flushAsync();
  return frame;
}

function createRuntimeHarness() {
  let parentWindow;
  const messageChannels = [];
  function HarnessMessageChannel() {
    const channel = new MessageChannel();
    messageChannels.push(channel);
    return channel;
  }
  const document = createDocument(() => createChildWindow(parentWindow));
  parentWindow = createWindow();
  parentWindow.document = document;
  parentWindow.crypto = webcrypto;
  parentWindow.dispatchMessage = function (event) {
    parentWindow.dispatch("message", event);
  };

  const fetchState = { bridgeRequests: [] };
  const parentContext = vm.createContext({
    MessageChannel: HarnessMessageChannel,
    TextEncoder,
    URL,
    btoa: (value) => Buffer.from(value, "binary").toString("base64"),
    clearInterval,
    clearTimeout,
    console,
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
    this.textContent = "";
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

  get classList() {
    return {
      add: (name) => {
        this.className = `${this.className} ${name}`.trim();
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
  if (url.endsWith("/manifest.json")) {
    return jsonResponse({
      id: "notes-lite",
      name: "Notes Lite",
      version: "0.1.0",
      description: "Budget warning probe app.",
      permissions: ["core.step", "storage.read"],
      storagePrefix: "notes-lite:",
      capabilities: [],
      dataVersion: "1.0.0",
      networkPolicy: { allow: [] },
      resourceBudget: { maxBridgeCallsPerMinute: 5 },
    });
  }
  if (url.endsWith("/index.html")) {
    return textResponse("<!doctype html><html><head></head><body><main data-testid=\"app-root\"></main></body></html>");
  }
  if (url === "/bridge") {
    const request = JSON.parse(options.body);
    state.bridgeRequests.push(request);
    return jsonResponse({
      id: request.id,
      ok: true,
      result: request.method === "runtime.capabilities" ? { runtimeVersion: "test" } : {},
    });
  }
  throw new Error(`Unexpected fetch URL in runtime harness: ${url}`);
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

async function flushAsync() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}
