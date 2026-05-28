#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PlatformDatabase } from "./fake-platform-host/src/platform-database.js";
import { examplesDir, repoRoot } from "./fake-platform-host/src/paths.js";
import { validatePackage } from "./fake-platform-host/src/package-validator.js";

const checks = [];

await runCheck("json.parse", checkJsonParse);
await runCheck("schema.fixtures", checkSchemaFixtures);
await runCheck("sqlite.migrate", checkSqliteMigrations);
await runCheck("postgres.static", checkPostgresSql);
await runCheck("examples.validate", checkExamplePackages);
await runCheck("manifests.sync", checkManifestSync);
await runCheck("spec.security_lint", checkSecurityLint);
await runCheck("plugin.mcp", checkPluginMcp);
await runCheck("runtime.static", checkRuntimeStatic);
await runCheck("server.static", checkServerStatic);
await runCheck("native.static", checkNativeStatic);

for (const check of checks) {
  console.log(`${check.ok ? "ok" : "fail"} ${check.name}${check.detail ? ` ${check.detail}` : ""}`);
}

const failed = checks.filter((check) => !check.ok);
if (failed.length > 0) {
  process.exitCode = 1;
}

async function runCheck(name, fn) {
  try {
    const detail = await fn();
    checks.push({ name, ok: true, detail });
  } catch (error) {
    checks.push({ name, ok: false, detail: error.message });
  }
}

function checkJsonParse() {
  const files = walk(repoRoot).filter((filePath) => filePath.endsWith(".json"));
  for (const filePath of files) {
    JSON.parse(fs.readFileSync(filePath, "utf8"));
  }
  return `files=${files.length}`;
}

function checkSchemaFixtures() {
  const validator = createSchemaValidator(path.join(repoRoot, "schemas"));
  const fixtureGroups = [
    ["manifest.schema.json", walk(repoRoot).filter((filePath) => path.basename(filePath) === "manifest.json" && isExamplePath(filePath))],
    ["app-migration.schema.json", walk(examplesDir).filter((filePath) => /\/migrations\/\d+_to_\d+\.json$/.test(filePath))],
    ["micro-test.schema.json", jsonFiles(path.join(repoRoot, "tests", "micro"))],
    ["micro-test.schema.json", jsonFiles(path.join(repoRoot, "tests", "accessibility")).filter((filePath) => filePath.endsWith(".microtest.json"))],
    ["mutation-fixture.schema.json", jsonFiles(path.join(repoRoot, "tests", "mutation")).filter((filePath) => filePath.endsWith(".mutation.json"))],
    ["bridge-contract-fixture.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "bridge"))],
    ["core-step.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "core"))],
    ["accessibility-report.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "accessibility"))],
    ["app-version-record.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "app-version"))],
    ["runtime-capabilities.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "capabilities"))],
    ["dev-control-response.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "control-plane"))],
    ["install-report.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "install-report"))],
    ["app-signature.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "signatures"))],
    ["runtime-snapshot.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "snapshots"))],
    ["db-app-records.schema.json", [path.join(repoRoot, "tests", "fixtures", "db", "app-install-records.fixture.json")]],
    ["backup-export.schema.json", [path.join(repoRoot, "tests", "fixtures", "db", "backup-export.fixture.json")]],
    ["db-runtime-records.schema.json", [path.join(repoRoot, "tests", "fixtures", "db", "runtime-records.fixture.json")]],
    ["db-test-records.schema.json", [path.join(repoRoot, "tests", "fixtures", "db", "test-records.fixture.json")]],
  ];

  let count = 0;
  for (const [schemaName, files] of fixtureGroups) {
    for (const filePath of files.filter((candidate) => fs.existsSync(candidate))) {
      const errors = validator.validate(readJson(filePath), schemaName);
      if (errors.length > 0) {
        throw new Error(`${relative(filePath)} failed ${schemaName}: ${errors.slice(0, 3).join("; ")}`);
      }
      count += 1;
    }
  }

  return `files=${count}`;
}

function checkSqliteMigrations() {
  const db = new PlatformDatabase();
  try {
    const requiredTables = [
      "apps",
      "app_versions",
      "app_files",
      "app_permissions",
      "app_installations",
      "app_storage",
      "runtime_sessions",
      "bridge_calls",
      "core_events",
      "core_actions",
      "runtime_snapshots",
      "control_sessions",
      "control_commands",
      "micro_tests",
      "test_runs",
      "network_mocks",
      "dialog_mocks",
      "app_migrations",
      "migration_runs",
      "app_install_reports",
      "backup_exports",
    ];
    const existing = new Set(db.all("SELECT name FROM sqlite_master WHERE type = 'table'").map((row) => row.name));
    const missing = requiredTables.filter((table) => !existing.has(table));
    if (missing.length > 0) {
      throw new Error(`missing tables: ${missing.join(", ")}`);
    }
    return `tables=${requiredTables.length}`;
  } finally {
    db.close();
  }
}

function checkPostgresSql() {
  const files = walk(path.join(repoRoot, "db", "postgres")).filter((filePath) => filePath.endsWith(".sql"));
  for (const filePath of files) {
    const sql = fs.readFileSync(filePath, "utf8");
    if (!/CREATE TABLE/i.test(sql)) {
      throw new Error(`${relative(filePath)} does not declare tables`);
    }
    if (/\bJSON\b/.test(sql) && !/\bJSONB\b/.test(sql)) {
      throw new Error(`${relative(filePath)} should use JSONB for logical JSON columns`);
    }
  }
  return `files=${files.length}`;
}

function checkExamplePackages() {
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  for (const app of apps) {
    const result = validatePackage(path.join(examplesDir, app));
    if (!result.ok) {
      throw new Error(`${app}: ${JSON.stringify(result.errors)}`);
    }
  }
  return `apps=${apps.length}`;
}

function checkManifestSync() {
  const rootExamples = path.join(repoRoot, "examples");
  if (!fs.existsSync(rootExamples)) {
    return "deprecated examples absent";
  }
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  for (const app of apps) {
    for (const fileName of ["manifest.json", "index.html", "styles.css", "app.js", "smoke-tests.json", "README.md"]) {
      const canonicalPath = path.join(examplesDir, app, fileName);
      const duplicatePath = path.join(rootExamples, app, fileName);
      if (!fs.existsSync(duplicatePath)) {
        throw new Error(`missing duplicate ${fileName} for ${app}`);
      }
      const canonical = fs.readFileSync(canonicalPath, "utf8");
      const duplicate = fs.readFileSync(duplicatePath, "utf8");
      if (canonical !== duplicate) {
        throw new Error(`example duplicate drift: ${app}/${fileName}`);
      }
    }
  }
  return `apps=${apps.length}`;
}

function checkSecurityLint() {
  const nativeFiles = walk(path.join(repoRoot, "native")).filter((filePath) => /\.(kt|java|swift|cs|cpp|cc|c|h|hpp|rs|js|ts)$/.test(filePath));
  for (const filePath of nativeFiles) {
    const source = fs.readFileSync(filePath, "utf8");
    if (source.includes("addJavascriptInterface")) {
      throw new Error(`forbidden addJavascriptInterface in ${relative(filePath)}`);
    }
    if (source.includes("SharedPreferences")) {
      throw new Error(`forbidden SharedPreferences persistence in ${relative(filePath)}`);
    }
  }
  const manifestFiles = walk(repoRoot).filter((filePath) => path.basename(filePath) === "manifest.json");
  for (const filePath of manifestFiles) {
    const manifest = readJson(filePath);
    if ("networkAllowlist" in manifest) {
      throw new Error(`removed networkAllowlist in ${relative(filePath)}`);
    }
  }
  return `nativeFiles=${nativeFiles.length} manifests=${manifestFiles.length}`;
}

function checkPluginMcp() {
  const pluginDir = path.join(repoRoot, "codex-plugin", "platform-control");
  const config = readJson(path.join(pluginDir, ".mcp.json"));
  const servers = Object.entries(config.mcp_servers ?? {});
  if (servers.length === 0) {
    throw new Error("codex plugin declares no MCP servers");
  }
  for (const [name, server] of servers) {
    const serverScript = server.args?.find((arg) => arg.endsWith("src/server.js"));
    if (!serverScript) {
      throw new Error(`${name} does not point at an MCP server script`);
    }
    const resolved = path.resolve(pluginDir, serverScript);
    if (!fs.existsSync(resolved)) {
      throw new Error(`${name} MCP script missing: ${path.relative(repoRoot, resolved)}`);
    }
  }
  return `servers=${servers.length}`;
}

function checkRuntimeStatic() {
  const source = fs.readFileSync(path.join(repoRoot, "runtime-web", "runtime.js"), "utf8");
  const required = [
    "new MessageChannel()",
    "window.AppRuntime = {",
    "capabilities: function",
    "validateRuntimeBridgeRequest",
    "validateMethodParams",
    "validateNetworkRequest",
    "validateAndRecordBudget",
    "permissionForBridgeMethod",
    "isKnownRuntimeBridgeMethod",
    "Bridge request contains unknown top-level fields",
    "permission_denied",
    "unknown_method",
    "network_policy_denied",
    "resource_budget_exceeded",
    "createMountToken",
    "mountsByFrame",
    "mountsByPort",
    "bridge.unauthorized_channel",
    '"x-app-id": portMount.appId',
    '"x-mount-token": portMount.mountToken',
    "body: JSON.stringify(request)",
  ];
  for (const snippet of required) {
    if (!source.includes(snippet)) {
      throw new Error(`runtime-web/runtime.js missing ${snippet}`);
    }
  }
  if (/message\s*=\s*\{[^}]*appId/s.test(source)) {
    throw new Error("runtime bridge request body must not include appId");
  }
  return "bridge=messagechannel,nonce-bound request=no-appid permission,policy,budget=runtime-preflight";
}

function checkServerStatic() {
  const source = fs.readFileSync(path.join(repoRoot, "server", "src", "main.zig"), "utf8");
  const required = [
    "POST\") and std.mem.eql(u8, parsed.path, \"/bridge\")",
    "POST\") and std.mem.eql(u8, parsed.path, \"/webapps/validate\")",
    "\"/webapps/examples/\"",
    "\"/webapps/examples.json\"",
    "fn handleBridge",
    "fn handleWebappValidate",
    "fn handleExampleAsset",
    "fn writeStatic",
    "fn validateBridgeRequest",
    "fn handleStorageBridge",
    "fn handleAppLogBridge",
    "fn handleDbControlEndpoint",
    "fn requireControlToken",
    "NATIVE_AI_SERVER_CONTROL_TOKEN",
    'headerValue(headers, "x-platform-control-token")',
    '"/db/snapshot"',
    '"/db/app-storage"',
    '"/db/bridge-calls"',
    '"/db/export-debug-bundle"',
    "fn dbSnapshotJson",
    "fn dbDebugBundleJson",
    "fn queryAppStorageRowsJson",
    "fn queryBridgeCallsRowsJson",
    "fn logAppMessage",
    "sqlite3_open",
    "app_storage",
    "runtime_sessions",
    "bridge_calls",
    "storage.get\\\":true",
    "app.log\\\":true",
    "Bridge request contains unknown top-level fields",
    'headerValue(headers, "x-app-id")',
    'headerValue(headers, "x-runtime-session-id")',
    'headerValue(headers, "x-mount-token")',
    "Bridge calls require a channel-derived mount token",
    "\"core.step\"",
    "\"runtime.capabilities\"",
    "\"bridge.unauthorized_channel\"",
    "\"platform_unsupported\"",
    "fn isKnownUnsupportedBridgeMethod",
    "fn hasInteractiveWithoutTestId",
    "\"missing_testid\"",
    "fn hasUnknownRuntimeBridgeCall",
    "fn isAllowedRuntimeBridgeMethod",
    "\"unknown_method\"",
  ];
  for (const snippet of required) {
    if (!source.includes(snippet)) {
      throw new Error(`server/src/main.zig missing ${snippet}`);
    }
  }
  return "bridge=core.step,runtime.capabilities,storage,app.log db=safe-token-gated unsupported=platform_unsupported validate=package-policy,testids,methods examples=static,json";
}

function checkNativeStatic() {
  const macBridge = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "WebBridge.swift"), "utf8");
  const macStorage = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "PlatformStorage.swift"), "utf8");
  const macNetwork = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "PlatformNetwork.swift"), "utf8");
  const iosBridge = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebBridge.swift"), "utf8");
  const iosHost = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebHostView.swift"), "utf8");
  const iosStorage = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformStorage.swift"), "utf8");
  const iosNetwork = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformNetwork.swift"), "utf8");
  const windowsHost = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "WebViewHost.cpp"), "utf8");
  const windowsBridge = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "WebBridge.cpp"), "utf8");
  const windowsStorage = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformStorage.cpp"), "utf8");
  const windowsNetwork = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformNetwork.cpp"), "utf8");
  const windowsCmake = fs.readFileSync(path.join(repoRoot, "native", "windows", "CMakeLists.txt"), "utf8");
  const linuxHost = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "webkit_host.c"), "utf8");
  const linuxBridge = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "web_bridge.c"), "utf8");
  const linuxStorage = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_storage.c"), "utf8");
  const linuxNetwork = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_network.c"), "utf8");
  const linuxMeson = fs.readFileSync(path.join(repoRoot, "native", "linux", "meson.build"), "utf8");
  const androidMain = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "MainActivity.kt"), "utf8");
  const androidBridge = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "NativeBridge.kt"), "utf8");
  const androidStorage = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformStorage.kt"), "utf8");
  const androidNetwork = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformNetwork.kt"), "utf8");
  const macRequired = [
    '"target": "macos"',
    '"devMode": true',
    '"limits":',
    '"network.request": true',
    '"core.step": false',
    "struct AppSandboxContext",
    "networkPolicy",
    "permissionForBridgeMethod",
    "approvedPermissions.contains(permission)",
  ];
  for (const snippet of macRequired) {
    if (!macBridge.includes(snippet)) {
      throw new Error(`macOS runtime.capabilities missing ${snippet}`);
    }
  }
  for (const snippet of ["request.context.appId", "request.context.storagePrefix", "storagePrefixFailure"]) {
    if (!macStorage.includes(snippet)) {
      throw new Error(`macOS storage missing context enforcement: ${snippet}`);
    }
  }
  if (macStorage.includes("appId(for:")) {
    throw new Error("macOS storage must not derive app id from storage key");
  }
  for (const snippet of ["URLSessionConfiguration.ephemeral", "network_policy_denied", "NetworkPolicyRule", "willPerformHTTPRedirection"]) {
    if (!macNetwork.includes(snippet)) {
      throw new Error(`macOS network missing policy enforcement: ${snippet}`);
    }
  }
  if (macNetwork.includes("platform_unsupported")) {
    throw new Error("macOS network.request must not remain a platform_unsupported stub");
  }
  if (macBridge.includes('"network.request": "native"') || macBridge.includes("pending-zig-link")) {
    throw new Error("macOS runtime.capabilities must use schema-shaped booleans");
  }
  const forbiddenAppLogPermissionChecks = [
    [macBridge, '"network.request", "core.step", "app.log"'],
    [iosBridge, '"network.request", "core.step", "app.log"'],
    [androidBridge, '"network.request", "core.step", "app.log" -> method'],
    [windowsBridge, 'method == L"network.request" || method == L"core.step" || method == L"app.log"'],
    [linuxBridge, 'g_strcmp0(method, "core.step") == 0 || g_strcmp0(method, "app.log") == 0'],
  ];
  for (const [source, snippet] of forbiddenAppLogPermissionChecks) {
    if (source.includes(snippet)) {
      throw new Error("native bridges must keep app.log permission-less");
    }
  }
  const iosRequired = [
    [iosBridge, "WKScriptMessageHandlerWithReply"],
    [iosHost, "contentController.addScriptMessageHandler"],
    [iosHost, "websiteDataStore = .nonPersistent()"],
    [iosBridge, '"target": "ios-simulator"'],
    [iosBridge, '"devMode": true'],
    [iosBridge, '"limits":'],
    [iosBridge, '"network.request": true'],
    [iosBridge, '"core.step": false'],
    [iosBridge, "struct AppSandboxContext"],
    [iosBridge, "networkPolicy"],
    [iosBridge, "permissionForBridgeMethod"],
    [iosBridge, "approvedPermissions.contains(permission)"],
    [iosStorage, "request.context.appId"],
    [iosStorage, "request.context.storagePrefix"],
    [iosStorage, "storagePrefixFailure"],
  ];
  for (const [source, snippet] of iosRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`iOS host missing ${snippet}`);
    }
  }
  if (iosStorage.includes("appId(for:")) {
    throw new Error("iOS storage must not derive app id from storage key");
  }
  for (const snippet of ["URLSessionConfiguration.ephemeral", "network_policy_denied", "NetworkPolicyRule", "willPerformHTTPRedirection"]) {
    if (!iosNetwork.includes(snippet)) {
      throw new Error(`iOS network missing policy enforcement: ${snippet}`);
    }
  }
  if (iosNetwork.includes("platform_unsupported")) {
    throw new Error("iOS network.request must not remain a platform_unsupported stub");
  }
  const windowsRequired = [
    [windowsHost, "SetVirtualHostNameToFolderMapping"],
    [windowsHost, "add_WebMessageReceived"],
    [windowsHost, "get_Source"],
    [windowsHost, "https://runtime.local.platform/"],
    [windowsHost, "SandboxContextFromSource"],
    [windowsBridge, "permissionForBridgeMethod"],
    [windowsBridge, "approvedPermissions.contains(permission"],
    [windowsBridge, 'features.Insert(L"network.request", json::JsonValue::CreateBooleanValue(true))'],
    [windowsHost, "NetworkPolicyForApp"],
    [windowsHost, ".networkPolicy"],
    [windowsStorage, "request.context.appId"],
    [windowsStorage, "request.context.storagePrefix"],
    [windowsStorage, "storagePrefixFailure"],
  ];
  for (const [source, snippet] of windowsRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Windows host missing ${snippet}`);
    }
  }
  if (windowsStorage.includes("appIdFor")) {
    throw new Error("Windows storage must not derive app id from storage key");
  }
  for (const snippet of ["WinHttpOpenRequest", "network_policy_denied", "NetworkPolicyRule", "WINHTTP_DISABLE_REDIRECTS"]) {
    if (!windowsNetwork.includes(snippet)) {
      throw new Error(`Windows network missing policy enforcement: ${snippet}`);
    }
  }
  if (windowsNetwork.includes("platform_unsupported")) {
    throw new Error("Windows network.request must not remain a platform_unsupported stub");
  }
  if (!windowsCmake.includes("winhttp")) {
    throw new Error("Windows network bridge must link winhttp");
  }
  const linuxRequired = [
    [linuxHost, "webkit_security_manager_register_uri_scheme_as_secure"],
    [linuxHost, "webkit_user_content_manager_register_script_message_handler"],
    [linuxHost, "script-message-received::NativeAIPlatformBridge"],
    [linuxHost, "app-runtime://runtime-web/index.html"],
    [linuxHost, "sandbox_context_from_uri"],
    [linuxHost, "network_policy_for_app"],
    [linuxHost, ".network_policy"],
    [linuxBridge, "permission_for_bridge_method"],
    [linuxBridge, "approved_permissions_contains"],
    [linuxBridge, '"network.request"'],
    [linuxStorage, "request->context.app_id"],
    [linuxStorage, "request->context.storage_prefix"],
    [linuxStorage, "storage_prefix_failure"],
  ];
  for (const [source, snippet] of linuxRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Linux host missing ${snippet}`);
    }
  }
  if (linuxStorage.includes("app_id_for_key")) {
    throw new Error("Linux storage must not derive app id from storage key");
  }
  for (const snippet of ["soup_session_send_and_read", "network_policy_denied", "NetworkPolicyRule", "SOUP_MESSAGE_NO_REDIRECT"]) {
    if (!linuxNetwork.includes(snippet)) {
      throw new Error(`Linux network missing policy enforcement: ${snippet}`);
    }
  }
  if (linuxNetwork.includes("platform_unsupported")) {
    throw new Error("Linux network.request must not remain a platform_unsupported stub");
  }
  if (!linuxMeson.includes("libsoup-3.0")) {
    throw new Error("Linux network bridge must link libsoup-3.0");
  }
  const androidRequired = [
    [androidMain, "WebViewCompat.addWebMessageListener"],
    [androidMain, "https://appassets.androidplatform.net"],
    [androidMain, "allowFileAccess = false"],
    [androidMain, "AssetRootPathHandler"],
    [androidMain, "sandboxContextFromManifest"],
    [androidMain, "NetworkPolicyRule.fromManifest"],
    [androidMain, "webapps/examples/$appId/manifest.json"],
    [androidMain, 'webView.loadUrl("https://appassets.androidplatform.net/runtime/index.html")'],
    [androidBridge, "permissionForBridgeMethod"],
    [androidBridge, "approvedPermissions.contains(permission)"],
    [androidBridge, '"network.request" to true'],
    [androidBridge, "networkPolicy"],
    [androidStorage, "SQLiteOpenHelper"],
    [androidStorage, "request.context.appId"],
    [androidStorage, "request.context.storagePrefix"],
  ];
  for (const [source, snippet] of androidRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Android host missing ${snippet}`);
    }
  }
  const androidGradle = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "build.gradle.kts"), "utf8");
  for (const snippet of ["syncNativeAiAssets", 'into("runtime")', 'into("webapps")', "assets.srcDir(generatedNativeAiAssets)"]) {
    if (!androidGradle.includes(snippet)) {
      throw new Error(`Android Gradle asset sync missing ${snippet}`);
    }
  }
  for (const snippet of ["HttpURLConnection", "network_policy_denied", "NetworkPolicyRule", "instanceFollowRedirects = false", "CountDownLatch"]) {
    if (!androidNetwork.includes(snippet)) {
      throw new Error(`Android network missing policy enforcement: ${snippet}`);
    }
  }
  if (androidNetwork.includes("platform_unsupported")) {
    throw new Error("Android network.request must not remain a platform_unsupported stub");
  }
  return "macos.capabilities=schema-shaped storage=context-enforced ios.webbridge=context-enforced windows.webview2=origin-checked linux.webkit=scheme-checked android.webmessage=origin-checked";
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function jsonFiles(root) {
  return walk(root).filter((filePath) => filePath.endsWith(".json"));
}

function isExamplePath(filePath) {
  const rel = relative(filePath);
  return rel.startsWith("webapps/examples/") || rel.startsWith("examples/");
}

function createSchemaValidator(schemaDir) {
  const schemaCache = new Map();

  function loadSchema(schemaName) {
    if (!schemaCache.has(schemaName)) {
      schemaCache.set(schemaName, readJson(path.join(schemaDir, schemaName)));
    }
    return schemaCache.get(schemaName);
  }

  function validate(value, schemaName) {
    const schema = loadSchema(schemaName);
    return validateValue(value, schema, "$", schema);
  }

  function validateValue(value, schema, valuePath, rootSchema) {
    if (!schema || Object.keys(schema).length === 0) return [];

    if (schema.$ref) {
      const { schema: resolved, root } = resolveRef(schema.$ref, rootSchema);
      return validateValue(value, resolved, valuePath, root);
    }

    if (schema.oneOf) {
      const matches = schema.oneOf.filter((candidate) => validateValue(value, candidate, valuePath, rootSchema).length === 0);
      return matches.length === 1 ? [] : [`${valuePath} must match exactly one schema option`];
    }

    const errors = [];
    if (schema.const !== undefined && !sameJson(value, schema.const)) {
      errors.push(`${valuePath} must equal ${JSON.stringify(schema.const)}`);
    }
    if (schema.enum && !schema.enum.some((allowed) => sameJson(value, allowed))) {
      errors.push(`${valuePath} must be one of ${schema.enum.map((item) => JSON.stringify(item)).join(", ")}`);
    }
    if (schema.type && !typeMatches(value, schema.type)) {
      errors.push(`${valuePath} must be ${Array.isArray(schema.type) ? schema.type.join(" or ") : schema.type}`);
      return errors;
    }
    if (typeof value === "string") {
      if (Number.isInteger(schema.minLength) && value.length < schema.minLength) errors.push(`${valuePath} is shorter than ${schema.minLength}`);
      if (Number.isInteger(schema.maxLength) && value.length > schema.maxLength) errors.push(`${valuePath} is longer than ${schema.maxLength}`);
      if (schema.pattern && !new RegExp(schema.pattern).test(value)) errors.push(`${valuePath} does not match ${schema.pattern}`);
      if (schema.format === "date-time" && Number.isNaN(Date.parse(value))) errors.push(`${valuePath} must be a date-time string`);
    }
    if (typeof value === "number") {
      if (typeof schema.minimum === "number" && value < schema.minimum) errors.push(`${valuePath} must be >= ${schema.minimum}`);
      if (typeof schema.maximum === "number" && value > schema.maximum) errors.push(`${valuePath} must be <= ${schema.maximum}`);
    }
    if (Array.isArray(value)) {
      if (Number.isInteger(schema.minItems) && value.length < schema.minItems) errors.push(`${valuePath} must contain at least ${schema.minItems} items`);
      if (Number.isInteger(schema.maxItems) && value.length > schema.maxItems) errors.push(`${valuePath} must contain at most ${schema.maxItems} items`);
      if (schema.uniqueItems) {
        const seen = new Set(value.map((item) => JSON.stringify(item)));
        if (seen.size !== value.length) errors.push(`${valuePath} must contain unique items`);
      }
      if (schema.items) {
        value.forEach((item, index) => errors.push(...validateValue(item, schema.items, `${valuePath}[${index}]`, rootSchema)));
      }
    }
    if (isPlainObject(value)) {
      const properties = schema.properties ?? {};
      for (const required of schema.required ?? []) {
        if (!(required in value)) errors.push(`${valuePath}.${required} is required`);
      }
      for (const [key, item] of Object.entries(value)) {
        if (key in properties) {
          errors.push(...validateValue(item, properties[key], `${valuePath}.${key}`, rootSchema));
        } else if (schema.additionalProperties === false) {
          errors.push(`${valuePath}.${key} is not allowed`);
        } else if (isPlainObject(schema.additionalProperties)) {
          errors.push(...validateValue(item, schema.additionalProperties, `${valuePath}.${key}`, rootSchema));
        }
      }
    }
    return errors;
  }

  function resolveRef(ref, rootSchema) {
    if (ref.startsWith("#/")) {
      return { schema: resolveJsonPointer(rootSchema, ref.slice(1)), root: rootSchema };
    }
    const [schemaName, pointer] = ref.split("#");
    const root = loadSchema(schemaName);
    return { schema: pointer ? resolveJsonPointer(root, pointer) : root, root };
  }

  return { validate };
}

function resolveJsonPointer(root, pointer) {
  return pointer
    .split("/")
    .filter(Boolean)
    .reduce((value, segment) => value?.[segment.replace(/~1/g, "/").replace(/~0/g, "~")], root);
}

function typeMatches(value, type) {
  const types = Array.isArray(type) ? type : [type];
  return types.some((candidate) => {
    if (candidate === "array") return Array.isArray(value);
    if (candidate === "object") return isPlainObject(value);
    if (candidate === "integer") return Number.isInteger(value);
    if (candidate === "number") return typeof value === "number";
    if (candidate === "null") return value === null;
    return typeof value === candidate;
  });
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function sameJson(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function walk(root) {
  const files = [];
  if (!fs.existsSync(root)) return files;
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    if (
      entry.name === ".git" ||
      entry.name === "node_modules" ||
      entry.name === ".gradle" ||
      entry.name === ".zig-cache" ||
      entry.name === ".build" ||
      entry.name === "build" ||
      entry.name === "zig-out"
    ) {
      continue;
    }
    const abs = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...walk(abs));
    } else if (entry.isFile()) {
      files.push(abs);
    }
  }
  return files;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath);
}

if (process.argv[1] !== fileURLToPath(import.meta.url)) {
  throw new Error("check-repo.mjs is meant to be executed directly");
}
