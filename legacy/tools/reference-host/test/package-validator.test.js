import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { examplesDir, repoRoot } from "../src/paths.js";
import { validatePackage, validateSourceSnippet } from "../src/package-validator.js";

test("all canonical example packages validate", () => {
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  assert.deepEqual(apps.sort(), ["api-dashboard", "calendar-planner", "core-replay-lab", "file-transformer", "notes-lite", "task-workbench", "test-camera"]);

  for (const app of apps) {
    const result = validatePackage(path.join(examplesDir, app));
    assert.equal(result.ok, true, `${app}: ${JSON.stringify(result.errors)}`);
    assert.equal(result.manifest.id, app);
  }
});

test("golden package fixtures validate as source packages", () => {
  const goldenPackage = JSON.parse(fs.readFileSync(path.join(repoRoot, "tests", "golden", "minimal-counter.package.json"), "utf8"));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "golden-package-"));
  for (const file of goldenPackage.files) {
    fs.writeFileSync(
      path.join(dir, file.path),
      file.path === "manifest.json" ? `${JSON.stringify(goldenPackage.manifest, null, 2)}\n` : file.content,
    );
  }

  const result = validatePackage(dir);
  assert.equal(result.ok, true, JSON.stringify(result.errors));
  assert.equal(result.manifest.id, "minimal-counter");
});

test("forbidden JS source snippets are rejected with policy codes", () => {
  const cases = [
    ["forbidden_network_api", "fetch('https://example.com')"],
    ["forbidden_network_api", "new XMLHttpRequest()"],
    ["forbidden_network_api", "new WebSocket('wss://example.com')"],
    ["forbidden_network_api", "new EventSource('https://example.com/events')"],
    ["forbidden_network_api", "navigator.sendBeacon('https://example.com/collect', '{}')"],
    ["forbidden_eval", "eval('1 + 1')"],
    ["forbidden_storage_api", "localStorage.setItem('x', 'y')"],
    ["forbidden_storage_api", "cookieStore.get('session')"],
    ["forbidden_sql_api", "const db = openDatabase('app', '1.0', 'app', 1024); db.transaction(tx => tx.executeSql('select 1'));"],
    ["forbidden_native_bridge", "webkit.messageHandlers.bridge.postMessage({})"],
    ["forbidden_native_bridge", "native.exec('open', {})"],
    ["forbidden_native_bridge", "TerranePlatformBridge.postMessage({})"],
    ["forbidden_appid_param", 'AppRuntime.call("storage.get", { appId: "other-app", key: "notes-lite:notes" })'],
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
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "reference-host-package-"));
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
    ["invalid_network_policy", { allow: [], allowCredentials: true }],
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
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "reference-host-package-"));
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

test("resource hint links are rejected", () => {
  const resourceHint = copyExamplePackage("notes-lite");
  const indexPath = path.join(resourceHint, "index.html");
  fs.writeFileSync(
    indexPath,
    fs.readFileSync(indexPath, "utf8").replace("</head>", '<link rel="preconnect" href="https://tracker.example"></head>'),
  );

  const result = validatePackage(resourceHint);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "forbidden_resource_hint"), true);
});

test("smoke test selectors must use data-testid", () => {
  const dir = copyExamplePackage("notes-lite");
  fs.writeFileSync(
    path.join(dir, "smoke-tests.json"),
    JSON.stringify([{ name: "brittle id selector", steps: [{ type: "click", selector: "#new-note" }] }], null, 2),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "invalid_smoke_selector"), true);
});

test("bundled smoke tests reject micro-test-only commands", () => {
  const dir = copyExamplePackage("notes-lite");
  fs.writeFileSync(
    path.join(dir, "smoke-tests.json"),
    JSON.stringify([
      {
        name: "uses a mock",
        steps: [{ tool: "runtime.network_mock_set", args: { match: { url: "https://example.test" } } }],
      },
    ], null, 2),
  );

  const result = validatePackage(dir);
  assert.equal(result.ok, false);
  assert.equal(result.errors.some((error) => error.code === "invalid_smoke_tests"), true);
});

test("external HTML resource URLs are rejected", () => {
  const cases = [
    '<img src="https://tracker.example/pixel.png" alt="">',
    '<a data-testid="external-link" href="https://example.test">External</a>',
    '<video src="//media.example.test/video.mp4"></video>',
    '<img srcset="small.png 1x, https://tracker.example/large.png 2x" alt="">',
  ];

  for (const snippet of cases) {
    const dir = copyExamplePackage("notes-lite");
    const indexPath = path.join(dir, "index.html");
    fs.writeFileSync(indexPath, fs.readFileSync(indexPath, "utf8").replace("</main>", `${snippet}</main>`));

    const result = validatePackage(dir);
    assert.equal(result.ok, false, snippet);
    assert.equal(result.errors.some((error) => error.code === "forbidden_external_resource"), true, snippet);
  }
});

test("stylesheet link must load plain styles.css exactly once", () => {
  const missingStylesheet = copyExamplePackage("notes-lite");
  const missingIndexPath = path.join(missingStylesheet, "index.html");
  fs.writeFileSync(
    missingIndexPath,
    fs.readFileSync(missingIndexPath, "utf8").replace('<link rel="stylesheet" href="styles.css">', ""),
  );
  const missingResult = validatePackage(missingStylesheet);
  assert.equal(missingResult.ok, false);
  assert.equal(missingResult.errors.some((error) => error.code === "missing_stylesheet"), true);

  const alternateStylesheet = copyExamplePackage("notes-lite");
  const alternateIndexPath = path.join(alternateStylesheet, "index.html");
  fs.writeFileSync(
    alternateIndexPath,
    fs.readFileSync(alternateIndexPath, "utf8").replace('href="styles.css"', 'href="theme.css"'),
  );
  const alternateResult = validatePackage(alternateStylesheet);
  assert.equal(alternateResult.ok, false);
  assert.equal(alternateResult.errors.some((error) => error.code === "forbidden_stylesheet_href"), true);

  const duplicateStylesheet = copyExamplePackage("notes-lite");
  const duplicateIndexPath = path.join(duplicateStylesheet, "index.html");
  fs.writeFileSync(
    duplicateIndexPath,
    fs.readFileSync(duplicateIndexPath, "utf8").replace("</head>", '<link rel="stylesheet" href="styles.css"></head>'),
  );
  const duplicateResult = validatePackage(duplicateStylesheet);
  assert.equal(duplicateResult.ok, false);
  assert.equal(duplicateResult.errors.some((error) => error.code === "invalid_stylesheet_count"), true);

  const nonPlainStylesheet = copyExamplePackage("notes-lite");
  const nonPlainIndexPath = path.join(nonPlainStylesheet, "index.html");
  fs.writeFileSync(
    nonPlainIndexPath,
    fs.readFileSync(nonPlainIndexPath, "utf8").replace('href="styles.css"', 'href="styles.css" media="print"'),
  );
  const nonPlainResult = validatePackage(nonPlainStylesheet);
  assert.equal(nonPlainResult.ok, false);
  assert.equal(nonPlainResult.errors.some((error) => error.code === "forbidden_stylesheet_attribute"), true);
});

test("inline styles and unsafe-inline style CSP are rejected", () => {
  const inlineStyle = copyExamplePackage("notes-lite");
  const inlineIndexPath = path.join(inlineStyle, "index.html");
  fs.writeFileSync(
    inlineIndexPath,
    fs.readFileSync(inlineIndexPath, "utf8").replace("<main", '<main style="color:red"'),
  );
  const inlineResult = validatePackage(inlineStyle);
  assert.equal(inlineResult.ok, false);
  assert.equal(inlineResult.errors.some((error) => error.code === "forbidden_inline_style"), true);

  const unsafeCsp = copyExamplePackage("notes-lite");
  const cspIndexPath = path.join(unsafeCsp, "index.html");
  fs.writeFileSync(
    cspIndexPath,
    fs.readFileSync(cspIndexPath, "utf8").replace("style-src 'self'", "style-src 'self' 'unsafe-inline'"),
  );
  const cspResult = validatePackage(unsafeCsp);
  assert.equal(cspResult.ok, false);
  assert.equal(cspResult.errors.some((error) => error.code === "forbidden_inline_style_csp"), true);
});

test("app script tag must load plain app.js exactly once", () => {
  const moduleScript = copyExamplePackage("notes-lite");
  const moduleIndexPath = path.join(moduleScript, "index.html");
  fs.writeFileSync(
    moduleIndexPath,
    fs.readFileSync(moduleIndexPath, "utf8").replace('<script src="app.js"></script>', '<script src="app.js" type="module"></script>'),
  );
  const moduleResult = validatePackage(moduleScript);
  assert.equal(moduleResult.ok, false);
  assert.equal(moduleResult.errors.some((error) => error.code === "forbidden_app_script_attribute"), true);

  const duplicateScript = copyExamplePackage("notes-lite");
  const duplicateIndexPath = path.join(duplicateScript, "index.html");
  fs.writeFileSync(
    duplicateIndexPath,
    fs.readFileSync(duplicateIndexPath, "utf8").replace("</body>", '<script src="app.js"></script></body>'),
  );
  const duplicateResult = validatePackage(duplicateScript);
  assert.equal(duplicateResult.ok, false);
  assert.equal(duplicateResult.errors.some((error) => error.code === "invalid_app_script_count"), true);

  const missingScript = copyExamplePackage("notes-lite");
  const missingIndexPath = path.join(missingScript, "index.html");
  fs.writeFileSync(
    missingIndexPath,
    fs.readFileSync(missingIndexPath, "utf8").replace('<script src="app.js"></script>', ""),
  );
  const missingResult = validatePackage(missingScript);
  assert.equal(missingResult.ok, false);
  assert.equal(missingResult.errors.some((error) => error.code === "missing_app_script"), true);
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
    ["forbidden_css_url", ".logo { background-image: url(icon.png); }"],
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

test("package validation enforces hard package and migration file caps", () => {
  const tooManyFilesDir = copyExamplePackage("notes-lite");
  fs.mkdirSync(path.join(tooManyFilesDir, "migrations"), { recursive: true });
  for (let index = 0; index < 29; index += 1) {
    fs.writeFileSync(path.join(tooManyFilesDir, "migrations", `extra-${index}.txt`), "");
  }
  const tooManyFiles = validatePackage(tooManyFilesDir);
  assert.equal(tooManyFiles.ok, false);
  assert.equal(tooManyFiles.errors.some((error) => error.code === "resource_budget_exceeded"), true);

  const tooManyMigrationsDir = copyExamplePackage("notes-lite");
  fs.mkdirSync(path.join(tooManyMigrationsDir, "migrations"), { recursive: true });
  for (let index = 0; index < 17; index += 1) {
    fs.writeFileSync(path.join(tooManyMigrationsDir, "migrations", `${index + 1}_to_${index + 2}.json`), JSON.stringify({
      appId: "notes-lite",
      fromDataVersion: index + 1,
      toDataVersion: index + 2,
      steps: [],
    }));
  }
  const tooManyMigrations = validatePackage(tooManyMigrationsDir);
  assert.equal(tooManyMigrations.ok, false);
  assert.equal(tooManyMigrations.errors.some((error) => error.code === "resource_budget_exceeded"), true);
});

test("platform-generated package artifacts are rejected", () => {
  for (const generatedFile of ["signature.json", "install-report.json", "content-hashes.json"]) {
    const dir = copyExamplePackage("notes-lite");
    fs.writeFileSync(path.join(dir, generatedFile), "{}");

    const result = validatePackage(dir);
    assert.equal(result.ok, false, generatedFile);
    assert.equal(result.errors.some((error) => error.code === "platform_generated_artifact"), true, generatedFile);
  }
});

function copyExamplePackage(name) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "reference-host-package-"));
  fs.cpSync(path.join(examplesDir, name), dir, { recursive: true });
  return dir;
}
