import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { examplesDir } from "../src/paths.js";
import { validatePackage, validateSourceSnippet } from "../src/package-validator.js";

test("all canonical example packages validate", () => {
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  assert.deepEqual(apps.sort(), ["api-dashboard", "core-replay-lab", "file-transformer", "notes-lite", "task-workbench"]);

  for (const app of apps) {
    const result = validatePackage(path.join(examplesDir, app));
    assert.equal(result.ok, true, `${app}: ${JSON.stringify(result.errors)}`);
    assert.equal(result.manifest.id, app);
  }
});

test("forbidden JS source snippets are rejected with policy codes", () => {
  const cases = [
    ["forbidden_network_api", "fetch('https://example.com')"],
    ["forbidden_network_api", "new XMLHttpRequest()"],
    ["forbidden_network_api", "new WebSocket('wss://example.com')"],
    ["forbidden_network_api", "new EventSource('https://example.com/events')"],
    ["forbidden_eval", "eval('1 + 1')"],
    ["forbidden_storage_api", "localStorage.setItem('x', 'y')"],
    ["forbidden_native_bridge", "webkit.messageHandlers.bridge.postMessage({})"],
    ["forbidden_native_bridge", "NativeAIPlatformBridge.postMessage({})"],
    ["forbidden_parent_access", "window.parent.postMessage({}, '*')"],
    ["forbidden_service_worker", "navigator.serviceWorker.register('/sw.js')"],
    ["forbidden_trusted_types_policy", "trustedTypes.createPolicy('app', {})"],
  ];

  for (const [code, source] of cases) {
    const result = validateSourceSnippet(source);
    assert.equal(result.ok, false);
    assert.equal(result.errors.some((error) => error.code === code), true, code);
  }
});

test("manifest.networkAllowlist is rejected", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.writeFileSync(path.join(dir, "index.html"), '<!doctype html><script src="app.js"></script>');
  fs.writeFileSync(path.join(dir, "styles.css"), "body { color: black; }");
  fs.writeFileSync(path.join(dir, "app.js"), "console.log('ok');");
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      id: "bad-app",
      name: "Bad App",
      version: "0.1.0",
      runtimeVersion: "0.1.0",
      dataVersion: 1,
      entry: "index.html",
      description: "Bad manifest",
      permissions: [],
      storagePrefix: "bad-app:",
      capabilities: { required: [], optional: [] },
      resourceBudget: {
        maxDomNodes: 10,
        maxStorageBytes: 10,
        maxBridgeCallsPerMinute: 10,
        maxNetworkRequestsPerMinute: 10,
        maxTimers: 10,
        maxLogLinesPerMinute: 10,
        maxPackageBytes: 100000,
        maxFileBytes: 100000,
      },
      networkPolicy: { allow: [] },
      networkAllowlist: [],
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "removed_manifest_field"), true);
});

test("bridge capabilities must be covered by permissions", () => {
  const dir = copyExamplePackage("notes-lite");
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.capabilities.required = [...manifest.capabilities.required, "network.request"];
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "invalid_capabilities"), true);
});

test("networkPolicy validates methods, headers, sizes, and timeout bounds", () => {
  const cases = [
    ["invalid_network_policy", { allow: [], allowCredentials: "yes" }],
    ["invalid_network_policy", { allow: [{ origin: "https://api.example.com", methods: ["GET"], allowedHeaders: ["x-debug", "x-debug"] }] }],
    ["invalid_network_policy", { allow: [{ origin: "https://api.example.com", methods: ["GET"], allowedHeaders: ["cookie"] }] }],
    ["invalid_network_policy", { allow: [{ origin: "https://api.example.com", methods: ["GET"], pathPrefix: 42 }] }],
    ["invalid_network_policy", { allow: [{ origin: "https://api.example.com", methods: ["GET"], maxResponseBytes: -1 }] }],
    ["invalid_network_policy", { allow: [{ origin: "https://api.example.com", methods: ["GET"], timeoutMs: 120001 }] }],
    ["invalid_network_methods", { allow: [{ origin: "https://api.example.com", methods: ["TRACE"] }] }],
    ["invalid_network_methods", { allow: [{ origin: "https://api.example.com", methods: ["GET", "GET"] }] }],
  ];

  for (const [code, networkPolicy] of cases) {
    const dir = copyExamplePackage("api-dashboard");
    const manifestPath = path.join(dir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.networkPolicy = networkPolicy;
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const result = validatePackage(dir);
    assert.equal(result.ok, false, code);
    assert.equal(result.errors.some((error) => error.code === code), true, `${code}: ${JSON.stringify(result.errors)}`);
  }
});

test("interactive HTML elements must declare data-testid", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.writeFileSync(path.join(dir, "index.html"), '<!doctype html><button id="go">Go</button><script src="app.js"></script>');
  fs.writeFileSync(path.join(dir, "styles.css"), "body { color: black; }");
  fs.writeFileSync(path.join(dir, "app.js"), "console.log('ok');");
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      id: "bad-app",
      name: "Bad App",
      version: "0.1.0",
      runtimeVersion: "0.1.0",
      dataVersion: 1,
      entry: "index.html",
      description: "Bad HTML",
      permissions: [],
      storagePrefix: "bad-app:",
      capabilities: { required: [], optional: [] },
      resourceBudget: {
        maxDomNodes: 10,
        maxStorageBytes: 10,
        maxBridgeCallsPerMinute: 10,
        maxNetworkRequestsPerMinute: 10,
        maxTimers: 10,
        maxLogLinesPerMinute: 10,
        maxPackageBytes: 100000,
        maxFileBytes: 100000,
      },
      networkPolicy: { allow: [] },
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "missing_testid"), true);
});

test("remote stylesheet links and CSS imports are rejected", () => {
  const remoteStylesheet = copyExamplePackage("notes-lite");
  const remoteIndexPath = path.join(remoteStylesheet, "index.html");
  fs.writeFileSync(
    remoteIndexPath,
    fs.readFileSync(remoteIndexPath, "utf8").replace('href="styles.css"', 'href="https://cdn.example.test/styles.css"'),
  );

  const remoteResult = validatePackage(remoteStylesheet);
  assert.equal(remoteResult.ok, false);
  assert.equal(remoteResult.errors.some((error) => error.code === "forbidden_remote_stylesheet"), true);

  const importStylesheet = copyExamplePackage("notes-lite");
  const cssPath = path.join(importStylesheet, "styles.css");
  fs.writeFileSync(cssPath, `@import url(https://cdn.example.test/theme.css);\n${fs.readFileSync(cssPath, "utf8")}`);

  const importResult = validatePackage(importStylesheet);
  assert.equal(importResult.ok, false);
  assert.equal(importResult.errors.some((error) => error.code === "forbidden_css_import"), true);
});

test("document policy rejects navigation and viewport escape hatches", () => {
  const htmlCases = [
    ["forbidden_meta_refresh", '<meta http-equiv="refresh" content="0;url=https://example.test">'],
    ["forbidden_base_href", '<base href="https://example.test/">'],
    ["forbidden_form_action", '<form action="https://example.test/submit"><button data-testid="submit-button">Send</button></form>'],
  ];

  for (const [code, snippet] of htmlCases) {
    const dir = copyExamplePackage("notes-lite");
    const indexPath = path.join(dir, "index.html");
    fs.writeFileSync(indexPath, fs.readFileSync(indexPath, "utf8").replace("</head>", `${snippet}</head>`));
    const result = validatePackage(dir);
    assert.equal(result.ok, false);
    assert.equal(result.errors.some((error) => error.code === code), true, code);
  }

  const cssCases = [
    ["forbidden_external_font", "@font-face { font-family: Bad; src: url(font.woff2); }"],
    ["forbidden_fixed_position", ".escape { position: fixed; inset: 0; }"],
  ];

  for (const [code, snippet] of cssCases) {
    const dir = copyExamplePackage("notes-lite");
    const cssPath = path.join(dir, "styles.css");
    fs.writeFileSync(cssPath, `${snippet}\n${fs.readFileSync(cssPath, "utf8")}`);
    const result = validatePackage(dir);
    assert.equal(result.ok, false);
    assert.equal(result.errors.some((error) => error.code === code), true, code);
  }
});

test("dataVersion increases require consecutive migration files", () => {
  const dir = copyExamplePackage("notes-lite");
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.dataVersion = 2;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "migration_missing"), true);
});

test("migration steps cannot escape the app storage prefix", () => {
  const dir = copyExamplePackage("notes-lite");
  const manifestPath = path.join(dir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.dataVersion = 2;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  fs.mkdirSync(path.join(dir, "migrations"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "migrations", "1_to_2.json"),
    JSON.stringify({
      appId: "notes-lite",
      fromDataVersion: 1,
      toDataVersion: 2,
      steps: [{ op: "renameKey", from: "notes-lite:notes", to: "other-app:notes" }],
    }),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "invalid_migration_prefix"), true);
});

function copyExamplePackage(name) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fake-host-package-"));
  fs.cpSync(path.join(examplesDir, name), dir, { recursive: true });
  return dir;
}
