import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { PlatformError } from "./errors.js";

const DEFAULT_TIMEOUT_MS = 7_500;
const STEP_SETTLE_MS = 75;

export class BrowserSmokeRunner {
  constructor({ database, dispatchBridge, chromePath = null, timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
    this.database = database;
    this.dispatchBridge = dispatchBridge;
    this.chromePath = chromePath;
    this.timeoutMs = timeoutMs;
  }

  static chromePath() {
    return findChromeExecutable();
  }

  static isAvailable() {
    return Boolean(findChromeExecutable());
  }

  async run({ appId, installId, tests, files }) {
    if (!this.database || typeof this.dispatchBridge !== "function") {
      throw new PlatformError("browser_smoke_unavailable", "Browser smoke runner requires a database and bridge dispatcher", {});
    }

    const chromePath = this.chromePath ?? findChromeExecutable();
    if (!chromePath) {
      throw new PlatformError("browser_smoke_unavailable", "No Chrome-compatible executable found for browser smoke tests", {
        env: "TERRANE_CHROME_PATH",
      });
    }

    const sessionId = this.database.createRuntimeSession({
      appId,
      metadata: { runner: "browser-smoke", installId: installId ?? null },
    });
    installDefaultSmokeMocks(this.database, { appId, sessionId });

    const mountToken = `browser_smoke_${randomUUID()}`;
    const server = await startSmokeServer({
      appId,
      sessionId,
      mountToken,
      files,
      dispatchBridge: this.dispatchBridge,
    });

    const chrome = await launchChrome(chromePath, { timeoutMs: this.timeoutMs });
    const failures = [];
    const consoleErrors = [];
    let bridgeCalls = [];

    try {
      const client = await CdpClient.connect(chrome.webSocketUrl);
      try {
        const page = await openPage(client, server.url, this.timeoutMs);
        client.onEvent((event) => {
          if (event.sessionId !== page.sessionId) return;
          if (event.method === "Runtime.exceptionThrown") {
            consoleErrors.push({
              code: "runtime.exception",
              message: event.params?.exceptionDetails?.text ?? "Unhandled browser exception",
            });
          }
          if (event.method === "Runtime.consoleAPICalled" && event.params?.type === "error") {
            consoleErrors.push({
              code: "console.error",
              message: (event.params.args ?? []).map((arg) => arg.value ?? arg.description ?? "").join(" "),
            });
          }
        });

        await waitForRuntimeIdle(client, page.sessionId, this.timeoutMs);
        for (const test of tests) {
          await runOneSmokeTest({
            client,
            sessionId: page.sessionId,
            test,
            failures,
            timeoutMs: this.timeoutMs,
          });
        }
        bridgeCalls = await evaluateValue(client, page.sessionId, "window.__smokeRuntime ? window.__smokeRuntime.calls : []");
        const runtimeErrors = await evaluateValue(client, page.sessionId, "window.__smokeRuntime ? window.__smokeRuntime.errors : []");
        for (const error of runtimeErrors) {
          failures.push({
            test: error.test ?? null,
            code: "bridge.call_failed",
            method: error.method,
            message: error.error?.message ?? error.message ?? "Bridge call failed",
            details: error.error ?? {},
          });
        }
        for (const error of consoleErrors) {
          failures.push({
            test: null,
            code: error.code,
            message: error.message,
          });
        }
      } finally {
        await client.close();
      }
    } finally {
      await chrome.close();
      await server.close();
    }

    return {
      ok: failures.length === 0,
      appId,
      total: tests.length,
      assertions: tests.reduce((count, test) => count + (test.steps?.length ?? 0) + Object.keys(test.expected ?? {}).length, 0),
      failures,
      runner: "browser",
      browser: {
        engine: "chrome-cdp",
        executable: path.basename(chromePath),
      },
      bridgeCalls: bridgeCalls.map((call) => ({ method: call.method, id: call.id })),
      sessionId,
    };
  }
}

async function runOneSmokeTest({ client, sessionId, test, failures, timeoutMs }) {
  for (const step of test.steps ?? []) {
    const stepResult = await runSmokeStep(client, sessionId, step);
    if (!stepResult.ok) {
      failures.push({ test: test.name, ...stepResult });
      continue;
    }
    await waitForRuntimeIdle(client, sessionId, timeoutMs);
  }

  if (test.expected?.textIncludes) {
    const found = await waitForText(client, sessionId, test.expected.textIncludes, timeoutMs);
    if (!found) {
      failures.push({ test: test.name, code: "text.not_found", text: test.expected.textIncludes });
    }
  }

  for (const method of test.expected?.bridgeCallsInclude ?? []) {
    const called = await waitForBridgeCall(client, sessionId, method, timeoutMs);
    if (!called) {
      failures.push({ test: test.name, code: "bridge.call_missing", method });
    }
  }
}

async function waitForBridgeCall(client, sessionId, method, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const called = await evaluateValue(
      client,
      sessionId,
      `Boolean(window.__smokeRuntime && window.__smokeRuntime.calls.some((call) => call.method === ${JSON.stringify(method)}))`,
    );
    if (called) return true;
    await delay(25);
  }
  return false;
}

async function runSmokeStep(client, sessionId, step) {
  if (!step || typeof step !== "object") {
    return { ok: false, code: "invalid_smoke_step", message: "Smoke test step must be an object" };
  }
  if (step.type === "click") {
    return evaluateFunction(
      client,
      sessionId,
      (selector) => {
        const element = document.querySelector(selector);
        if (!element) return { ok: false, code: "selector.not_found", selector };
        element.click();
        return { ok: true };
      },
      [step.selector],
    );
  }
  if (step.type === "fill" || step.type === "select") {
    return evaluateFunction(
      client,
      sessionId,
      (selector, value) => {
        const element = document.querySelector(selector);
        if (!element) return { ok: false, code: "selector.not_found", selector };
        element.focus();
        element.value = value;
        element.dispatchEvent(new Event("input", { bubbles: true }));
        element.dispatchEvent(new Event("change", { bubbles: true }));
        return { ok: true };
      },
      [step.selector, String(step.value ?? "")],
    );
  }
  return { ok: false, code: "unknown_smoke_step", type: step.type };
}

async function startSmokeServer({ appId, sessionId, mountToken, files, dispatchBridge }) {
  const html = injectSmokeBootstrap(appId, files.get("index.html") ?? "");
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://127.0.0.1");
      if (req.method === "GET" && url.pathname === "/") {
        return sendText(res, 200, html, "text/html");
      }
      if (req.method === "GET" && url.pathname.startsWith("/app/")) {
        const filePath = decodeURIComponent(url.pathname.slice("/app/".length));
        const content = files.get(filePath);
        if (content == null) {
          return sendJson(res, 404, { ok: false, error: { code: "not_found", message: "File not found", details: { filePath } } });
        }
        return sendText(res, 200, content, contentType(filePath));
      }
      if (req.method === "POST" && url.pathname === "/bridge") {
        const request = await readBodyJson(req);
        const response = await dispatchBridge(request, { appId, sessionId, mountToken });
        return sendJson(res, 200, response);
      }
      return sendJson(res, 404, { ok: false, error: { code: "not_found", message: "Route not found", details: {} } });
    } catch (error) {
      return sendJson(res, 500, {
        ok: false,
        error: { code: error.code ?? "browser_smoke_server_error", message: error.message, details: error.details ?? {} },
      });
    }
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  return {
    url: `http://127.0.0.1:${address.port}/`,
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
      }),
  };
}

function injectSmokeBootstrap(appId, html) {
  const harnessHtml = html.replace(/<meta\b[^>]*http-equiv=["']Content-Security-Policy["'][^>]*>/i, "");
  const bootstrap = `<base href="/app/">
<script>
(function () {
  var nextId = 1;
  var handlers = new Map();
  var knownEvents = new Set(["runtime.ready", "runtime.suspend", "runtime.resume", "app.error", "app.budget_warning", "app.permission_revoked"]);
  window.__smokeRuntime = { calls: [], errors: [], pending: 0 };
  function emit(eventName, payload) {
    var set = handlers.get(eventName);
    if (!set) return;
    Array.from(set).forEach(function (handler) {
      try { handler(payload); } catch (error) { console.error(error); }
    });
  }
  function call(method, params) {
    var id = "browser_smoke_req_" + nextId++;
    var request = { id: id, method: method, params: params == null ? {} : params, timestamp: Date.now() };
    window.__smokeRuntime.calls.push({ id: id, method: method, params: request.params });
    window.__smokeRuntime.pending += 1;
    return fetch("/bridge", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request)
    }).then(function (response) {
      return response.json();
    }).then(function (response) {
      if (response.ok) return response.result;
      window.__smokeRuntime.errors.push({ id: id, method: method, error: response.error });
      emit("app.error", response.error);
      var error = new Error(response.error && response.error.message ? response.error.message : "Bridge call failed");
      error.code = response.error && response.error.code;
      error.details = response.error && response.error.details;
      throw error;
    }).catch(function (error) {
      if (!error || !error.code) {
        window.__smokeRuntime.errors.push({ id: id, method: method, message: error && error.message ? error.message : String(error) });
      }
      throw error;
    }).finally(function () {
      window.__smokeRuntime.pending -= 1;
    });
  }
  window.AppRuntime = {
    call: call,
    capabilities: function () { return call("runtime.capabilities", {}); },
    on: function (eventName, handler) {
      if (!knownEvents.has(eventName) || typeof handler !== "function") return function () {};
      if (!handlers.has(eventName)) handlers.set(eventName, new Set());
      var set = handlers.get(eventName);
      set.add(handler);
      return function () { set.delete(handler); };
    }
  };
  window.addEventListener("error", function (event) {
    window.__smokeRuntime.errors.push({ method: null, message: event.message || "Unhandled browser error" });
  });
  window.addEventListener("unhandledrejection", function (event) {
    var reason = event.reason || {};
    window.__smokeRuntime.errors.push({ method: null, message: reason.message || String(reason), error: reason });
  });
  Promise.resolve().then(function () {
    emit("runtime.ready", { appId: ${JSON.stringify(appId)}, runtimeVersion: "0.1.0" });
  });
})();
</script>`;

  if (/<head[^>]*>/i.test(harnessHtml)) {
    return harnessHtml.replace(/<head([^>]*)>/i, `<head$1>${bootstrap}`);
  }
  return `${bootstrap}${harnessHtml}`;
}

function installDefaultSmokeMocks(database, { appId, sessionId }) {
  for (const method of ["GET", "POST", "PUT", "PATCH", "DELETE"]) {
    database.addNetworkMock({
      sessionId,
      appId,
      method,
      urlPattern: "*",
      response: {
        status: 200,
        headers: { "content-type": "application/json" },
        bodyText: JSON.stringify({ ok: true, runner: "browser-smoke" }),
      },
    });
  }
  database.addDialogMock({
    sessionId,
    appId,
    dialogType: "openFile",
    response: {
      files: [{ name: "smoke.txt", mime: "text/plain", size: 11, text: "hello world" }],
    },
  });
  database.addDialogMock({
    sessionId,
    appId,
    dialogType: "saveFile",
    response: { ok: true },
  });
}

async function launchChrome(chromePath, { timeoutMs }) {
  const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-browser-smoke-"));
  const args = [
    "--headless=new",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    "--disable-background-networking",
    "--disable-default-apps",
    "--disable-extensions",
    "--disable-gpu",
    "--disable-sync",
    "--no-first-run",
    "--no-default-browser-check",
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ];
  if (process.platform === "linux") {
    args.unshift("--no-sandbox");
  }

  const child = spawn(chromePath, args, { stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  const webSocketUrl = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(
        new PlatformError("browser_smoke_unavailable", "Timed out waiting for Chrome DevTools endpoint", {
          chromePath,
          stderr: stderr.slice(-1_000),
        }),
      );
    }, timeoutMs);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(new PlatformError("browser_smoke_unavailable", error.message, { chromePath }));
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
      const match = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (match) {
        clearTimeout(timer);
        resolve(match[1]);
      }
    });
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      reject(
        new PlatformError("browser_smoke_unavailable", "Chrome exited before exposing DevTools", {
          chromePath,
          code,
          signal,
          stderr: stderr.slice(-1_000),
        }),
      );
    });
  });

  return {
    webSocketUrl,
    close: async () => {
      child.kill("SIGTERM");
      await waitForExit(child, 1_000);
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
        await waitForExit(child, 1_000);
      }
      fs.rmSync(userDataDir, { recursive: true, force: true });
    },
  };
}

class CdpClient {
  static async connect(webSocketUrl) {
    const socket = new WebSocket(webSocketUrl);
    const client = new CdpClient(socket);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    return client;
  }

  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    this.eventWaiters = [];
    this.eventHandlers = [];
    socket.addEventListener("message", (event) => this.handleMessage(event));
    socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) {
        pending.reject(new PlatformError("browser_smoke_unavailable", "Chrome DevTools connection closed", {}));
      }
      this.pending.clear();
    });
  }

  send(method, params = {}, sessionId = null) {
    const id = this.nextId++;
    const message = sessionId ? { id, sessionId, method, params } : { id, method, params };
    this.socket.send(JSON.stringify(message));
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
  }

  waitForEvent(method, { sessionId = null, timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
    return new Promise((resolve, reject) => {
      const waiter = { method, sessionId, resolve, reject };
      waiter.timer = setTimeout(() => {
        this.eventWaiters = this.eventWaiters.filter((candidate) => candidate !== waiter);
        reject(new PlatformError("browser_smoke_timeout", `Timed out waiting for ${method}`, { method }));
      }, timeoutMs);
      this.eventWaiters.push(waiter);
    });
  }

  onEvent(handler) {
    this.eventHandlers.push(handler);
  }

  handleMessage(event) {
    const message = JSON.parse(event.data);
    if (message.id && this.pending.has(message.id)) {
      const pending = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(new PlatformError("browser_smoke_cdp_error", message.error.message, message.error));
      } else {
        pending.resolve(message.result ?? {});
      }
      return;
    }

    for (const handler of this.eventHandlers) {
      handler(message);
    }
    const waiter = this.eventWaiters.find(
      (candidate) => candidate.method === message.method && (!candidate.sessionId || candidate.sessionId === message.sessionId),
    );
    if (waiter) {
      clearTimeout(waiter.timer);
      this.eventWaiters = this.eventWaiters.filter((candidate) => candidate !== waiter);
      waiter.resolve(message.params ?? {});
    }
  }

  close() {
    this.socket.close();
    return Promise.resolve();
  }
}

async function openPage(client, url, timeoutMs) {
  const target = await client.send("Target.createTarget", { url: "about:blank" });
  const attached = await client.send("Target.attachToTarget", { targetId: target.targetId, flatten: true });
  const sessionId = attached.sessionId;
  await client.send("Page.enable", {}, sessionId);
  await client.send("Runtime.enable", {}, sessionId);
  const loaded = client.waitForEvent("Page.loadEventFired", { sessionId, timeoutMs });
  await client.send("Page.navigate", { url }, sessionId);
  await loaded;
  return { targetId: target.targetId, sessionId };
}

async function waitForRuntimeIdle(client, sessionId, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const idle = await evaluateValue(
      client,
      sessionId,
      "document.readyState === 'complete' && (!window.__smokeRuntime || window.__smokeRuntime.pending === 0)",
    );
    if (idle) {
      await delay(STEP_SETTLE_MS);
      return true;
    }
    await delay(25);
  }
  return false;
}

async function waitForText(client, sessionId, text, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const found = await evaluateFunction(
      client,
      sessionId,
      (expected) => document.body && document.body.innerText.includes(expected),
      [text],
    );
    if (found) return true;
    await delay(25);
  }
  return false;
}

async function evaluateValue(client, sessionId, expression) {
  const result = await client.send(
    "Runtime.evaluate",
    { expression, awaitPromise: true, returnByValue: true, userGesture: true },
    sessionId,
  );
  if (result.exceptionDetails) {
    throw new PlatformError("browser_smoke_eval_error", result.exceptionDetails.text, result.exceptionDetails);
  }
  return result.result?.value;
}

async function evaluateFunction(client, sessionId, fn, args = []) {
  return evaluateValue(client, sessionId, `(${fn.toString()})(...${JSON.stringify(args)})`);
}

function sendJson(res, status, value) {
  sendText(res, status, JSON.stringify(value, null, 2), "application/json");
}

function sendText(res, status, body, contentTypeValue) {
  res.writeHead(status, {
    "content-type": `${contentTypeValue}; charset=utf-8`,
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
}

async function readBodyJson(req) {
  let body = "";
  for await (const chunk of req) {
    body += chunk;
  }
  return body ? JSON.parse(body) : {};
}

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html";
  if (filePath.endsWith(".css")) return "text/css";
  if (filePath.endsWith(".js")) return "text/javascript";
  if (filePath.endsWith(".json")) return "application/json";
  return "text/plain";
}

function findChromeExecutable() {
  const explicit = process.env.TERRANE_CHROME_PATH;
  if (explicit && fs.existsSync(explicit)) return explicit;

  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/snap/bin/chromium",
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }

  for (const bin of ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "msedge"]) {
    const resolved = findOnPath(bin);
    if (resolved) return resolved;
  }
  return null;
}

function findOnPath(bin) {
  for (const entry of (process.env.PATH ?? "").split(path.delimiter)) {
    if (!entry) continue;
    const candidate = path.join(entry, bin);
    if (fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    delay(timeoutMs),
  ]);
}
