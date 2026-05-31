#!/usr/bin/env node
import { execFileSync } from "node:child_process";
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
await runCheck("examples.canonical", checkCanonicalExamples);
await runCheck("spec.security_lint", checkSecurityLint);
await runCheck("ci.workflow", checkCiWorkflow);
await runCheck("performance.harness", checkPerformanceHarness);
await runCheck("release.packaging", checkReleasePackaging);
await runCheck("plugin.mcp", checkPluginMcp);
await runCheck("control.openapi", checkControlOpenApi);
await runCheck("control.tools", checkControlToolContract);
await runCheck("fake-host.static", checkFakeHostStatic);
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
    ["micro-test.schema.json", jsonFiles(path.join(repoRoot, "tests", "golden")).filter((filePath) => filePath.endsWith(".golden.json"))],
    ["app-package.schema.json", jsonFiles(path.join(repoRoot, "tests", "golden")).filter((filePath) => filePath.endsWith(".package.json"))],
    ["mutation-fixture.schema.json", jsonFiles(path.join(repoRoot, "tests", "mutation")).filter((filePath) => filePath.endsWith(".mutation.json"))],
    ["bridge-contract-fixture.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "bridge"))],
    ["core-step.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "core"))],
    ["accessibility-report.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "accessibility"))],
    ["app-version-record.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "app-version"))],
    ["runtime-capabilities.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "capabilities"))],
    ["dev-control-command.schema.json", [path.join(repoRoot, "tests", "fixtures", "control-plane", "dev-control-command.fixture.json")]],
    ["dev-control-response.schema.json", jsonFiles(path.join(repoRoot, "tests", "fixtures", "control-plane")).filter((filePath) => !filePath.endsWith("dev-control-command.fixture.json"))],
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
      "fault_injections",
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
    const runtimeColumns = new Set(db.all("PRAGMA table_info(runtime_sessions)").map((row) => row.name));
    if (!runtimeColumns.has("resource_high_water_json")) {
      throw new Error("runtime_sessions missing resource_high_water_json");
    }
    return `tables=${requiredTables.length},runtime=resource-high-water`;
  } finally {
    db.close();
  }
}

function checkPostgresSql() {
  const sqliteDir = path.join(repoRoot, "db", "sqlite");
  const postgresDir = path.join(repoRoot, "db", "postgres");
  const files = walk(postgresDir).filter((filePath) => filePath.endsWith(".sql"));
  const sqliteSchema = parseSqlSchema(sqlText(sqliteDir));
  const postgresSchema = parseSqlSchema(sqlText(postgresDir));
  const missingTables = [...sqliteSchema.keys()].filter((table) => !postgresSchema.has(table));
  if (missingTables.length > 0) {
    throw new Error(`Postgres schema missing tables: ${missingTables.join(", ")}`);
  }
  for (const [table, sqliteColumns] of sqliteSchema) {
    const postgresColumns = postgresSchema.get(table) ?? new Set();
    const missingColumns = [...sqliteColumns].filter((column) => !postgresColumns.has(column));
    if (missingColumns.length > 0) {
      throw new Error(`Postgres schema ${table} missing columns: ${missingColumns.join(", ")}`);
    }
  }
  const postgresText = sqlText(postgresDir);
  if (!/PRIMARY KEY\s*\(\s*app_id\s*,\s*key\s*\)/i.test(postgresText)) {
    throw new Error("Postgres app_storage must keep PRIMARY KEY (app_id, key)");
  }
  for (const filePath of files) {
    const sql = fs.readFileSync(filePath, "utf8");
    if (!/CREATE TABLE/i.test(sql)) {
      throw new Error(`${relative(filePath)} does not declare tables`);
    }
    if (/\bJSON\b/.test(sql) && !/\bJSONB\b/.test(sql)) {
      throw new Error(`${relative(filePath)} should use JSONB for logical JSON columns`);
    }
  }
  if (process.env.POSTGRES_TEST_URL) {
    applyPostgresMigrations(process.env.POSTGRES_TEST_URL, postgresText);
    return `files=${files.length},tables=${postgresSchema.size},live=applied`;
  }
  return `files=${files.length},tables=${postgresSchema.size},live=skipped`;
}

function sqlText(dir) {
  return walk(dir)
    .filter((filePath) => filePath.endsWith(".sql"))
    .sort()
    .map((filePath) => fs.readFileSync(filePath, "utf8"))
    .join("\n");
}

function parseSqlSchema(sql) {
  const schema = new Map();
  const tablePattern = /CREATE\s+TABLE\s+IF\s+NOT\s+EXISTS\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([\s\S]*?)\);/gi;
  let match;
  while ((match = tablePattern.exec(sql))) {
    const table = match[1];
    const columns = new Set();
    for (const rawLine of match[2].split("\n")) {
      const line = rawLine.trim().replace(/,$/, "");
      if (!line || line.startsWith("--")) continue;
      const column = line.split(/\s+/)[0]?.replace(/"/g, "");
      if (!column || /^(PRIMARY|FOREIGN|CONSTRAINT|CHECK|UNIQUE|KEY)$/i.test(column)) continue;
      columns.add(column);
    }
    schema.set(table, columns);
  }
  return schema;
}

function applyPostgresMigrations(url, sql) {
  const schema = `native_ai_schema_check_${process.pid}_${Date.now()}`;
  const wrapped = `BEGIN; CREATE SCHEMA ${schema}; SET search_path TO ${schema}; ${sql}; ROLLBACK;`;
  execFileSync("psql", [url, "-v", "ON_ERROR_STOP=1", "-q", "-c", wrapped], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
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

function checkCanonicalExamples() {
  const rootExamples = path.join(repoRoot, "examples");
  if (fs.existsSync(rootExamples)) {
    throw new Error("deprecated root examples/ tree must not be restored; use webapps/examples/");
  }
  const apps = fs.readdirSync(examplesDir).filter((entry) => fs.statSync(path.join(examplesDir, entry)).isDirectory());
  return `webapps/examples apps=${apps.length}`;
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

function checkCiWorkflow() {
  const workflow = fs.readFileSync(path.join(repoRoot, ".github", "workflows", "ci.yml"), "utf8");
  const required = [
    "mlugg/setup-zig@v2",
    "version: 0.15.2",
    "libsqlite3-dev",
    "working-directory: zig-core",
    "working-directory: server",
    "zig build test",
    "tools/check-repo.mjs",
    "tools/fake-platform-host",
    "tests/performance/fake-host-latency.mjs --warmup 5 --samples 20 --out performance_runs --enforce-targets",
    "actions/upload-artifact@v4",
    "fake-host-performance-runs",
    "tools/codex-platform-mcp",
    "tools/package-release.mjs --out artifacts",
    "static-release-artifacts",
    "zig-core-release-artifacts",
    "tools/package-release.mjs --out artifacts --build-zig-core",
    "server-release-artifacts",
    "tools/package-release.mjs --out artifacts --build-server",
    "macos-native-release-artifacts",
    "tools/package-release.mjs --out artifacts --build-native-macos",
    "linux-native-release-artifacts",
    "Linux Native Release Artifacts",
    "libgtk-4-dev",
    "libwebkitgtk-6.0-dev",
    "libjson-glib-dev",
    "libsoup-3.0-dev",
    "meson",
    "ninja-build",
    "pkg-config",
    "tools/package-release.mjs --out artifacts --build-native-linux",
    "windows-native-release-artifacts",
    "tools/package-release.mjs --out artifacts --build-native-windows",
    "linux-native-smoke",
    "Linux Native Smoke (Docker)",
    "Docker-backed Linux native launch smoke",
    "tools/run-linux-native-docker.mjs",
    "macos-native-smoke",
    "NATIVE_AI_MACOS_SMOKE_LAUNCH",
    "ios-simulator-smoke",
    "NATIVE_AI_IOS_SMOKE_LAUNCH",
    "android-emulator-smoke",
    "android-actions/setup-android@v3",
    "gradle/actions/setup-gradle@v4",
    "gradle-version: 8.10.2",
    "reactivecircus/android-emulator-runner@v2",
    "NATIVE_AI_ANDROID_SMOKE_LAUNCH=1",
    "windows-native-smoke",
    "NATIVE_AI_WINDOWS_SMOKE_LAUNCH",
    "NATIVE_AI_WEBVIEW2_NUGET_DIR",
  ];
  for (const snippet of required) {
    if (!workflow.includes(snippet)) {
      throw new Error(`CI workflow missing ${snippet}`);
    }
  }
  return "node=24,zig=0.15.2,sqlite=yes,core=zig-test,server=zig-test,perf=target-enforced-smoke,release=static/zig-core/server/macos-native/linux-native/windows-native,native=linux-docker/macos/ios/android/windows-smoke";
}

function checkReleasePackaging() {
  const script = fs.readFileSync(path.join(repoRoot, "tools", "package-release.mjs"), "utf8");
  const docs = fs.readFileSync(path.join(repoRoot, "docs", "12_RELEASE_AND_CI.md"), "utf8");
  const toolsReadme = fs.readFileSync(path.join(repoRoot, "tools", "README.md"), "utf8");
  const ignore = fs.readFileSync(path.join(repoRoot, ".gitignore"), "utf8");
  const test = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "test", "release-packaging.test.js"), "utf8");
  const linuxNativeTest = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "test", "linux-native-build.test.js"), "utf8");
  const requiredScriptSnippets = [
    "runtime-web.zip",
    "example-webapps.zip",
    "release-manifest.json",
    "writeStoredZip",
    "buildZigCoreArtifacts",
    "buildServerArtifacts",
    "buildMacOSNativeArtifacts",
    "buildLinuxNativeArtifacts",
    "buildLinuxZigCoreSo",
    "buildWindowsNativeArtifacts",
    "windowsWebView2SdkStatus",
    "--build-zig-core",
    "--build-server",
    "--build-native-macos",
    "--build-native-linux",
    "--build-native-windows",
    "sha256",
    "server-executable",
    "native-host-app",
    "ZIG_CORE_TARGETS",
    "ios-arm64-device",
    "windows-x86_64",
    "zig_core.lib",
    "native-ai-server",
    "NativeAIHostMac.app",
    "LINUX_HOST_EXECUTABLE_NAME",
    "LINUX_HOST_APP_DIR_NAME",
    "native-ai-webapp-host",
    "libzig_core.so",
    '"resources", "runtime"',
    '"resources", "webapps", "examples"',
    '"resources", "db", "sqlite"',
    "webkitgtk-6.0",
    "NativeAIWebappHost.exe",
    "NativeAIWebappHost",
    "resources/db/sqlite/001_initial.sql",
  ];
  for (const snippet of requiredScriptSnippets) {
    if (!script.includes(snippet)) {
      throw new Error(`tools/package-release.mjs missing ${snippet}`);
    }
  }
  for (const snippet of [
    "tools/package-release.mjs --out artifacts --build-zig-core --build-server",
    "linux-x86_64/native-ai-server",
    "native-apps/macos/macos-arm64/NativeAIHostMac.app",
    "tools/package-release.mjs --out artifacts --build-native-macos",
    "native-apps/linux/linux-x86_64/NativeAIWebappHost",
    "native-ai-webapp-host",
    "libzig_core.so",
    "resources/runtime/",
    "resources/webapps/examples/",
    "resources/db/sqlite/",
    "tools/package-release.mjs --out artifacts --build-native-linux",
    "native-apps/windows/windows-x86_64/NativeAIWebappHost",
    "resources/db/sqlite/",
    "tools/package-release.mjs --out artifacts --build-native-windows",
    "tools/run-linux-native-docker.mjs",
    "release-manifest.json",
  ]) {
    if (!docs.includes(snippet)) {
      throw new Error(`docs/12 release artifacts missing ${snippet}`);
    }
  }
  if (!ignore.includes("artifacts/")) {
    throw new Error(".gitignore must ignore generated release artifacts");
  }
  for (const snippet of ["run-linux-native-docker", "--build-native-linux", "--build-native-windows", "tools/run-linux-native-docker.mjs", "linux/amd64"]) {
    if (!toolsReadme.includes(snippet)) {
      throw new Error(`tools/README missing ${snippet}`);
    }
  }
  const linuxDockerHelper = fs.readFileSync(path.join(repoRoot, "tools", "run-linux-native-docker.mjs"), "utf8");
  for (const snippet of ["defaultPlatform", "process.arch === \"x64\" ? \"\" : \"linux/amd64\""]) {
    if (!linuxDockerHelper.includes(snippet)) {
      throw new Error(`Linux Docker helper missing ${snippet}`);
    }
  }
  for (const snippet of [
    "listZipEntries",
    "buildZigCore: true",
    "buildServer: true",
    "buildNativeMacOS: true",
    "linuxReleaseSkipReason",
    "buildNativeLinux: true",
    "buildNativeWindows: true",
    "server-executable",
    "native-host-app",
    "native-ai-webapp-host",
    "libzig_core.so",
    "native-apps/linux/linux-x86_64/NativeAIWebappHost",
    "NativeAIWebappHost.exe",
    "zig_core.dll",
    "resources/db/sqlite/001_initial.sql",
    "runtime-web/index.html",
    "webapps/examples/notes-lite/manifest.json",
  ]) {
    if (!test.includes(snippet)) {
      throw new Error(`release packaging test missing ${snippet}`);
    }
  }
  for (const snippet of [
    "Linux packaged native artifact launches from executable-relative resources",
    "packageReleaseArtifacts({ outDir, buildNativeLinux: true })",
    "NATIVE_AI_ZIG_CORE_SO",
    "outside-repo-cwd",
    "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_SET_OK",
    "NATIVE_AI_LINUX_SMOKE_BRIDGE_STORAGE_GET_OK",
    "NATIVE_AI_LINUX_SMOKE_BRIDGE_CORE_STEP_OK",
    "resources/runtime/index.html",
    "resources/db/sqlite/001_initial.sql",
  ]) {
    if (!linuxNativeTest.includes(snippet)) {
      throw new Error(`Linux native packaged smoke missing ${snippet}`);
    }
  }
  return "artifacts=runtime-web.zip,example-webapps.zip,zig-core-libs,server-executable,macos-native-host,linux-native-host,windows-native-host,manifest";
}

function checkPerformanceHarness() {
  const harnessPath = path.join(repoRoot, "tests", "performance", "fake-host-latency.mjs");
  const source = fs.readFileSync(harnessPath, "utf8");
  const required = [
    "DEFAULT_WARMUP = 50",
    "DEFAULT_SAMPLES = 500",
    "runtime.storage_get",
    "runtime.storage_set",
    "runtime.core_step",
    "runtime_launcher_initial_load",
    "platform.list_webapps",
    "platform.open_webapp",
    "bridge_throughput",
    "open_all_examples_memory",
    "large_list",
    "network_timeout",
    "install_uninstall_loop",
    "DEFAULT_LIFECYCLE_LOOPS = 50",
    "DEFAULT_THROUGHPUT_CALLS = 1200",
    "p50",
    "p95",
    "performance_runs",
    "--enforce-targets",
    "--enforce-variance",
  ];
  for (const snippet of required) {
    if (!source.includes(snippet)) {
      throw new Error(`tests/performance/fake-host-latency.mjs missing ${snippet}`);
    }
  }
  return "fake-host-latency warmup=50 samples=500 lifecycle=50 throughput=1200 metrics=launcher,open,switch,storage,core scenarios=network-timeout,bridge-throughput,open-all-memory,large-list,install-uninstall p50/p95";
}

function checkPluginMcp() {
  const pluginDir = path.join(repoRoot, "codex-plugin", "platform-control");
  const pluginManifest = readJson(path.join(pluginDir, ".codex-plugin", "plugin.json"));
  const config = readJson(path.join(pluginDir, ".mcp.json"));
  const mcpConfigSource = fs.readFileSync(path.join(pluginDir, ".mcp.json"), "utf8");
  if (mcpConfigSource.includes("PLATFORM_CONTROL_TOKEN") || mcpConfigSource.includes("dev-token-change-me")) {
    throw new Error("codex plugin MCP config must not check in a shared control token");
  }
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
  const tokenHelper = fs.readFileSync(path.join(repoRoot, "tools", "control-token.js"), "utf8");
  const mcpConfig = fs.readFileSync(path.join(repoRoot, "tools", "codex-platform-mcp", "src", "config.js"), "utf8");
  const mcpServer = fs.readFileSync(path.join(repoRoot, "tools", "codex-platform-mcp", "src", "server.js"), "utf8");
  for (const snippet of ["PLATFORM_CONTROL_TOKEN_FILE", "control.token", "Control token file not found", "DEFAULT_CONTROL_URL"]) {
    if (!(mcpConfig.includes(snippet) || tokenHelper.includes(snippet))) {
      throw new Error(`codex MCP config missing token-file behavior: ${snippet}`);
    }
  }
  if (!mcpServer.includes("resolveControlConfig") || mcpServer.includes("dev-token-change-me")) {
    throw new Error("codex MCP server must resolve token-file config and avoid hardcoded tokens");
  }
  if (!mcpServer.includes("validateToolArguments")) {
    throw new Error("codex MCP server must validate tool arguments before forwarding");
  }
  const skillsDir = path.resolve(pluginDir, pluginManifest.skills ?? "skills");
  const requiredSkills = [
    ["platform-micro-test", "runtime.run_microtest"],
    ["generated-webapp-repair", "platform.run_policy_audit"],
    ["core-replay-debug", "runtime.replay_events"],
  ];
  for (const [skillName, requiredTool] of requiredSkills) {
    const skillPath = path.join(skillsDir, skillName, "SKILL.md");
    if (!fs.existsSync(skillPath)) {
      throw new Error(`codex plugin skill missing: ${path.relative(repoRoot, skillPath)}`);
    }
    const skillSource = fs.readFileSync(skillPath, "utf8");
    if (!skillSource.includes(`name: ${skillName}`) || !skillSource.includes(requiredTool)) {
      throw new Error(`${skillName} skill must declare its name and reference ${requiredTool}`);
    }
  }
  return `servers=${servers.length},skills=${requiredSkills.length}`;
}

function checkControlOpenApi() {
  const spec = readJson(path.join(repoRoot, "devtools", "control-plane", "openapi.json"));
  const paths = spec.paths ?? {};
  const requiredPostPaths = [
    "/command",
    "/db/snapshot",
    "/db/app-storage",
    "/db/app-versions",
    "/db/bridge-calls",
    "/db/core-events",
    "/db/test-runs",
    "/db/export-backup",
    "/db/import-backup",
    "/db/export-debug-bundle",
  ];
  for (const route of requiredPostPaths) {
    if (!paths[route]?.post) {
      throw new Error(`control-plane OpenAPI missing POST ${route}`);
    }
  }
  for (const route of requiredPostPaths.filter((route) => route.startsWith("/db/"))) {
    if (paths[route].post["x-dev-only"] !== true) {
      throw new Error(`control-plane OpenAPI ${route} must be marked x-dev-only`);
    }
  }
  if (Object.keys(paths).some((route) => /\bsql\b/i.test(route))) {
    throw new Error("control-plane OpenAPI must not expose arbitrary SQL endpoints");
  }
  return `dbPaths=${requiredPostPaths.filter((route) => route.startsWith("/db/")).length},raw-sql=absent`;
}

function mcpToolNames() {
  const contract = fs.readFileSync(path.join(repoRoot, "tools", "codex-platform-mcp", "src", "tool-contract.js"), "utf8");
  return [...contract.matchAll(/^\s*"([a-z0-9_.]+)",?/gm)].map((match) => match[1]);
}

function duplicates(values) {
  const seen = new Set();
  const duplicate = new Set();
  for (const value of values) {
    if (seen.has(value)) duplicate.add(value);
    seen.add(value);
  }
  return [...duplicate].sort();
}

function assertSameList(label, actual, expected) {
  const missing = expected.filter((value) => !actual.includes(value));
  const extra = actual.filter((value) => !expected.includes(value));
  const sameOrder = actual.length === expected.length && actual.every((value, index) => value === expected[index]);
  if (missing.length > 0 || extra.length > 0 || !sameOrder) {
    throw new Error(`${label} drift: missing=${missing.join(",") || "none"} extra=${extra.join(",") || "none"} order=${sameOrder ? "ok" : "changed"}`);
  }
}

function checkControlToolContract() {
  const toolNames = mcpToolNames();
  const contractSource = fs.readFileSync(path.join(repoRoot, "tools", "codex-platform-mcp", "src", "tool-contract.js"), "utf8");
  if (toolNames.length === 0) {
    throw new Error("MCP tool contract declares no tools");
  }
  const duplicateTools = duplicates(toolNames);
  if (duplicateTools.length > 0) {
    throw new Error(`duplicate MCP tool names: ${duplicateTools.join(", ")}`);
  }

  const commandSchema = readJson(path.join(repoRoot, "schemas", "dev-control-command.schema.json"));
  const schemaTools = commandSchema.properties?.tool?.enum ?? [];
  assertSameList("dev-control-command.schema.json tool enum", schemaTools, toolNames);

  const fakeHostSource = [
    "tools/fake-platform-host/src/fake-host.js",
    "tools/fake-platform-host/src/test-runner.js",
    "tools/fake-platform-host/src/platform-database.js",
  ]
    .map((relativePath) => fs.readFileSync(path.join(repoRoot, relativePath), "utf8"))
    .join("\n");
  const serverSource = fs.readFileSync(path.join(repoRoot, "server", "src", "main.zig"), "utf8");
  const fakeMissing = toolNames.filter((name) => !fakeHostSource.includes(`"${name}"`));
  const serverMissing = toolNames.filter((name) => !serverSource.includes(`"${name}"`));
  if (fakeMissing.length > 0) {
    throw new Error(`fake host missing MCP tools: ${fakeMissing.join(", ")}`);
  }
  if (serverMissing.length > 0) {
    throw new Error(`server missing MCP tools: ${serverMissing.join(", ")}`);
  }
  for (const snippet of ["inputSchemaFor", "validateToolArguments", "CONFIRM_TRUE", "platform.uninstall_webapp", "runtime.storage_set"]) {
    if (!contractSource.includes(snippet)) {
      throw new Error(`MCP tool contract missing typed argument support: ${snippet}`);
    }
  }
  return `tools=${toolNames.length},schema=fixed,args=validated,fake-host=covered,server=covered`;
}

function checkFakeHostStatic() {
  const fakeHost = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "fake-host.js"), "utf8");
  const fakeServer = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "server.js"), "utf8");
  const bridgeDispatcher = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "bridge-dispatcher.js"), "utf8");
  const core = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "core.js"), "utf8");
  const capabilities = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "capabilities.js"), "utf8");
  const packageValidator = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "package-validator.js"), "utf8");
  const testRunner = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "test-runner.js"), "utf8");
  const browserRunner = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "src", "browser-smoke-runner.js"), "utf8");
  const bridgeFixturesTest = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "test", "bridge-fixtures.test.js"), "utf8");
  const required = [
    [fakeHost, "new BrowserSmokeRunner"],
    [fakeHost, 'runner: args.runner ?? args.mode'],
    [bridgeDispatcher, "assertRuntimeCompatibility"],
    [bridgeDispatcher, "runtime_version_incompatible"],
    [bridgeDispatcher, "network.request private network targets are denied"],
    [bridgeDispatcher, "function isPrivateNetworkHost"],
    [core, "validateCoreEvent"],
    [core, "invalid_event"],
    [bridgeFixturesTest, "assertDeepSubset"],
    [bridgeFixturesTest, "resultSubset"],
    [bridgeFixturesTest, "errorDetailsSubset"],
    [testRunner, "NATIVE_AI_SMOKE_RUNNER"],
    [testRunner, 'runner: "static"'],
    [testRunner, 'from: "browser"'],
    [bridgeDispatcher, "assertAppLogParams"],
    [bridgeDispatcher, "app.log requires message"],
    [fakeHost, "queryConsoleLogs"],
    [fakeHost, "queryNotifications"],
    [fakeHost, "console_errors_found"],
    [fakeHost, "function resetAppIdArg"],
    [fakeHost, "requires confirm: true"],
    [browserRunner, "class BrowserSmokeRunner"],
    [browserRunner, "Chrome DevTools"],
    [browserRunner, "chrome-cdp"],
    [browserRunner, "window.AppRuntime"],
    [browserRunner, "window.__smokeRuntime.calls"],
    [browserRunner, "dispatchBridge(request"],
    [fakeServer, "generateControlToken"],
    [fakeServer, "writeControlTokenFile"],
    [fakeServer, "controlTokenPath"],
    [fakeHost, "controlAuthFailures"],
    [fakeHost, "control_connection_banned"],
    [fakeHost, "retryAfterSeconds"],
    [fakeHost, "serveRuntimeIndex"],
    [fakeHost, "__APP_RUNTIME_DEVTOOLS_ENABLED__"],
    [capabilities, 'platform: "fake"'],
    [capabilities, 'target: "fake-host"'],
    [capabilities, "devMode: true"],
    [capabilities, '"storage.read": true'],
    [capabilities, '"runtime.snapshot": true'],
    [capabilities, '"runtime.capabilities": true'],
    [capabilities, '"storage.get": true'],
    [capabilities, '"storage.set": true'],
    [packageValidator, "MAX_PACKAGE_FILES"],
    [packageValidator, "MAX_MIGRATION_FILES"],
    [packageValidator, "forbidden_sql_api"],
    [packageValidator, "sendBeacon"],
    [packageValidator, "cookieStore"],
  ];
  for (const [source, snippet] of required) {
    if (!source.includes(snippet)) {
      throw new Error(`fake-host browser smoke support missing ${snippet}`);
    }
  }
  return "smoke=static,browser-cdp bridge=runtime-compatible core=validated-events control-token=file auth-ban=audited";
}

function checkRuntimeStatic() {
  const source = fs.readFileSync(path.join(repoRoot, "runtime-web", "runtime.js"), "utf8");
  const required = [
    "new MessageChannel()",
    "window.AppRuntime = {",
    "capabilities: function",
    "knownEvents",
    "runtime.ready",
    "runtime.event",
    "app.error",
    "app.budget_warning",
    "function on(eventName, handler)",
    "emitAppError",
    "maybeWarnRuntimeBudget",
    "Bridge params must not include appId; app id is channel-derived",
    "core.step app field does not match the channel-derived app id",
    "validateRuntimeBridgeRequest",
    "validateMethodParams",
    "validateNetworkRequest",
    "network.request credentials are not allowed",
    "network.request private network targets are denied",
    "function isPrivateNetworkHost",
    "validateAndRecordBudget",
    "installBudgetGuards",
    "MutationObserver",
    "maxDomNodes",
    "maxTimers",
    "dispatchBridgeRequest",
    "__APP_RUNTIME_DEV_MOCK__",
    "__APP_RUNTIME_DEVTOOLS__",
    "__APP_RUNTIME_DEVTOOLS_ENABLED__",
    "runtimeDevtoolsEnabled",
    "runtimeDevtoolsSnapshot",
    "runtimeDevtoolsStorageSnapshot",
    "runtimeDevtoolsCoreEventLog",
    "delete window.__APP_RUNTIME_DEVTOOLS__",
    "dispatchDevMockBridgeRequest",
    "webkitNativeBridgeHandler",
    "androidNativeBridgeHandler",
    "webview2NativeBridgeHandler",
    "NativeAIPlatformBridge",
    "window.chrome && window.chrome.webview",
    "addEventListener(\"message\"",
    "handler.onmessage",
    "normalizeHostBridgeResponse",
    "invalid_response",
    "permissionForBridgeMethod",
    "isKnownRuntimeBridgeMethod",
    "Bridge request contains unknown top-level fields",
    "permission_denied",
    "unknown_method",
    "network_policy_denied",
    "resource_budget_exceeded",
    "createMountToken",
    "GENERATED_APP_CSP",
    "script-src 'self' app-runtime:",
    'frame.setAttribute("allow", "")',
    'frame.setAttribute("sandbox", "allow-scripts")',
    'frame.setAttribute("csp", GENERATED_APP_CSP)',
    "frame.srcdoc = srcdoc",
    "mountsByFrame",
    "mountsByPort",
    "portsByMountToken",
    "emitRuntimeEvent",
    "bridge.unauthorized_channel",
    "Bridge message arrived outside the assigned MessageChannel",
    '"x-app-id": mount.appId',
    '"x-mount-token": mount.mountToken',
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
  if (source.includes("<base href=")) {
    throw new Error("runtime generated app srcdoc must not inject base href");
  }
  if (source.includes("allow-same-origin")) {
    throw new Error("runtime generated app iframes must not use allow-same-origin");
  }
  if (/on:\s*function\s*\(\)\s*\{\s*return function \(\) \{\};\s*\}/s.test(source)) {
    throw new Error("runtime AppRuntime.on must not be a no-op");
  }
  return "bridge=messagechannel,nonce-bound,iframe-csp,webkit,android,webview2 request=no-appid permission,policy,budget=runtime-preflight,dom-timer-guards";
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
    "fn handleDialogBridge",
    "fn handleNotificationToastBridge",
    "fn handleNetworkRequestBridge",
    "fn handleAppLogBridge",
    "fn bridgePermissionApproved",
    "fn permissionForBridgeMethod",
    "fn enforceBridgeResourceBudget",
    "fn resourceBudgetLimit",
    "fn countBridgeCallsSince",
    "fn storageBytesAfterSet",
    "fn bridgeRuntimeCompatible",
    "resource_budget_exceeded",
    "maxBridgeCallsPerMinute",
    "maxNetworkRequestsPerMinute",
    "maxLogLinesPerMinute",
    "maxStorageBytes",
    "fn networkPolicyAllowsRequest",
    "fn isPrivateNetworkHost",
    "fn networkMockResultJsonAlloc",
    "fn insertNetworkMockControl",
    "fn resetNetworkMocksControl",
    "fn dialogMockResultJsonAlloc",
    "fn insertDialogMockControl",
    "fn handleControlCommand",
    "fn enforceProductionStartupRules",
    "fn isDevControlPath",
    "NATIVE_AI_SERVER_ENV",
    "production_control_disabled",
    "--allow-unsigned-dev",
    "--allow-runtime-mismatch",
    "--control-plane-port",
    "--token-file",
    "fn handleDbControlEndpoint",
    "fn requireControlToken",
    "fn authorizeControlRequest",
    "ControlAuthTracker",
    "control_connection_banned",
    "retryAfterSeconds",
    "fn initControlToken",
    "fn generateControlToken",
    "fn writeControlTokenFile",
    "control.token",
    "PLATFORM_CONTROL_TOKEN_FILE",
    "NATIVE_AI_SERVER_CONTROL_TOKEN",
    'headerValue(headers, "x-platform-control-token")',
    '"/control/command"',
    '"/webapps/install"',
    '"/rollback"',
    '"/packages/sign"',
    "fn handlePackageControlEndpoint",
    "fn controlToolForPackagePath",
    "fn appIdFromRollbackPath",
    '"platform.health"',
    '"platform.list_targets"',
    '"platform.open_webapp"',
    '"platform.reset_webapp"',
    '"platform.validate_package"',
    '"platform.sign_webapp_package"',
    '"platform.install_webapp_package"',
    '"platform.rollback_webapp"',
    '"platform.uninstall_webapp"',
    '"platform.approve_webapp_update"',
    '"platform.quarantine_webapp"',
    '"platform.create_snapshot"',
    '"platform.restore_snapshot"',
    '"platform.run_platform_smoke"',
    '"platform.run_repair_loop"',
    '"platform.migration_dry_run"',
    '"platform.migration_apply"',
    '"platform.list_webapps"',
    '"platform.install_report"',
    '"runtime.network_mock_set"',
    '"runtime.network_mock_reset"',
    '"runtime.dialog_mock_set"',
    '"runtime.storage_get"',
    '"runtime.storage_set"',
    '"runtime.storage_reset"',
    '"runtime.snapshot"',
    '"runtime.query"',
    '"runtime.click"',
    '"runtime.type"',
    '"runtime.set_value"',
    '"runtime.press_key"',
    '"runtime.drag"',
    '"runtime.wait_for"',
    '"runtime.screenshot"',
    '"runtime.resource_usage"',
    '"runtime.console_logs"',
    '"runtime.event_log"',
    '"runtime.clear_logs"',
    '"runtime.notification_capture"',
    '"runtime.timer_advance"',
    '"runtime.fault_inject"',
    '"runtime.call_bridge"',
    '"runtime.core_step"',
    '"runtime.core_snapshot"',
    '"runtime.replay_events"',
    '"runtime.assert_storage"',
    '"runtime.assert_visible"',
    '"runtime.assert_text"',
    '"runtime.accessibility_snapshot"',
    '"runtime.run_accessibility_audit"',
    '"runtime.assert_accessibility"',
    '"runtime.run_smoke_tests"',
    '"runtime.run_microtest"',
    '"runtime.assert_bridge_call"',
    '"runtime.assert_core_action"',
    '"runtime.compare_snapshot"',
    '"runtime.assert_no_console_errors"',
    '"db.query_app_storage"',
    '"db.query_app_versions"',
    '"db.query_core_events"',
    '"db.query_test_runs"',
    '"db.export_backup"',
    '"db.import_backup"',
    '"/db/snapshot"',
    '"/db/app-storage"',
    '"/db/bridge-calls"',
    '"/db/export-debug-bundle"',
    "fn dbSnapshotJson",
    "fn dbBackupExportJson",
    "fn importBackupControl",
    "fn insertBackupImportRecord",
    "fn dbDebugBundleJson",
    "fn signWebappPackage",
    "fn serverSignatureJsonAlloc",
    "ed25519",
    "fn signaturePayloadAlloc",
    "fn serverSigningKeyPair",
    "NATIVE_AI_SERVER_SIGNING_SEED",
    "fn installWebappPackage",
    "fn runtimeCompatibilityJsonAlloc",
    "fn runtimeVersionsCompatible",
    "fn allowRuntimeMismatch",
    "runtime_version_incompatible",
    "BEGIN IMMEDIATE",
    "fn insertAppVersion",
    "fn insertAppFile",
    "fn insertAppPermissions",
    "fn insertInstallReport",
    "fn evaluateSmokeTestsAlloc",
    "fn insertSmokeTestRun",
    "zig-server-static-smoke",
    "smoke-tests.json",
    "quarantined",
    "rolled-back",
    "fn rollbackWebappPackage",
    "fn uninstallWebappControl",
    "fn approveWebappUpdateControl",
    "fn quarantineWebappControl",
    "fn insertLifecycleInstallationEvent",
    "fn insertRollbackInstallationEvent",
    "rollback_data_version_incompatible",
    "fn restoreSnapshotStorageIntoDb",
    "dataRollbackSnapshotId",
    "fn createRuntimeSnapshot",
    "fn restoreRuntimeSnapshot",
    "fn insertRuntimeSnapshot",
    "fn restoreSnapshotStorage",
    "fn snapshotResourceUsageJsonAlloc",
    "fn snapshotContentHashByIdAlloc",
    "fn runtimeSnapshotControl",
    "fn runtimeQueryControl",
    "fn runtimeTargetControl",
    "fn runtimeScreenshotControl",
    "fn assertRuntimeVisibleControl",
    "fn assertRuntimeTextControl",
    "fn runtimeAccessibilitySnapshotControl",
    "fn runtimeAccessibilityAuditControl",
    "fn runtimeAssertAccessibilityControl",
    "fn htmlAccessibilityAuditJsonAlloc",
    "fn runtimeRunSmokeTestsControl",
    "fn runtimeRunMicrotestControl",
    "fn platformRunSmokeControl",
    "fn platformRunRepairLoopControl",
    "fn recordControlTestRun",
    "fn htmlDataTestIdsJsonAlloc",
    "fn runStorageMigration",
    "fn previewStorageMigration",
    "fn applyPackagedMigrationChainForInstall",
    "fn findPackageFileContent",
    "fn insertAppMigrationRecord",
    "fn insertMigrationRun",
    "fn applyMigrationChanges",
    "migrations/{d}_to_{d}.json",
    "invalid_migration",
    "action, previous_install_id",
    "fn insertInstallationEvent",
    "fn queryRowsJson",
    "fn queryAppStorageRowsJson",
    "fn queryAppVersionsRowsJson",
    "fn queryBridgeCallsRowsJson",
    "fn queryCoreEventsRowsJson",
    "fn queryTestRunsRowsJson",
    "fn runtimeEventLogControl",
    "fn consoleLogsControl",
    "fn notificationCaptureControl",
    "fn timerAdvanceControl",
    "fn insertFaultInjectionControl",
    "fn takeInjectedFaultAlloc",
    "fn clearRuntimeLogsControl",
    "fn callBridgeControl",
    "fn coreStepControl",
    "fn coreSnapshotControl",
    "fn replayEventsControl",
    "fn assertStorageControl",
    "fn assertBridgeCallControl",
    "fn assertCoreActionControl",
    "fn compareSnapshotControl",
    "fn canonicalJsonValueAlloc",
    "fn bridgeOkJsonAlloc",
    "fn bridgeErrorResponseJsonAlloc",
    "fn bridgeControlErrorResponse",
    "fn openWebappControl",
    "fn resetWebappControl",
    "reset requires confirm: true",
    "fn appendJsonColumnValue",
    "fn ensureAppRecord",
    "fn logBridgeCall",
    "fn recordBackupExport",
    "fn sha256HexAlloc",
    "contentHash",
    "fn recordCoreStep",
    "fn insertCoreEvent",
    "fn insertCoreActions",
    "fn coreStateVersionBefore",
    "core_event_",
    "core_action_",
    "fn auditControlCommand",
    "fn ensureServerControlSession",
    "fn controlToolForDbPath",
    "fn bindNullableText",
    "server-control-audit",
    "fn logAppMessage",
    "sqlite3_open",
    "app_versions",
    "app_files",
    "app_permissions",
    "app_installations",
    "app_storage",
    "runtime_sessions",
    "resource_high_water_json",
    "bridge_calls",
    "core_events",
    "runtime_snapshots",
    "control_sessions",
    "control_commands",
    "test_runs",
    "fault_injections",
    "app_migrations",
    "migration_runs",
    "app_install_reports",
    "backup_exports",
    "fn assertServerRequiredCapabilitiesAvailable",
    "\"storage.read\\\":true",
    "\"storage.write\\\":true",
    "storage.get\\\":true",
    "dialog.openFile\\\":true",
    "dialog.saveFile\\\":true",
    "notification.toast\\\":true",
    "network.request\\\":true",
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
    "\"network.request credentials are not allowed\"",
    "\"Bridge params must not include appId; app id is channel-derived\"",
    "fn isKnownUnsupportedBridgeMethod",
    "fn hasInteractiveWithoutTestId",
    "\"missing_testid\"",
    "fn validateServerPackageFileList",
    "\"unexpected_package_path\"",
    "\"platform_generated_artifact\"",
    "fn isPlatformGeneratedPackagePath",
    "max_package_files",
    "max_migration_files",
    "fn isKnownPackagePermission",
    "\"unknown_permission\"",
    "fn validateServerResourceBudget",
    "fn validateServerPackageBudget",
    "\"resource_budget_exceeded\"",
    "fn validateServerContentRating",
    "\"missing_content_rating\"",
    "\"invalid_content_rating\"",
    "fn validateServerMigrations",
    "\"migration_missing\"",
    "\"invalid_migration_prefix\"",
    "fn validateServerHtmlPolicy",
    "fn validateServerCsp",
    "fn validateServerScriptTags",
    "\"missing_app_script\"",
    "\"invalid_app_script_count\"",
    "\"forbidden_app_script_src\"",
    "\"forbidden_app_script_attribute\"",
    "fn validateServerStylesheetLinks",
    "\"forbidden_inline_script\"",
    "\"forbidden_inline_style\"",
    "\"forbidden_inline_style_csp\"",
    "\"forbidden_inline_script_csp\"",
    "fn htmlHasInlineStyle",
    "\"forbidden_stylesheet_href\"",
    "\"forbidden_resource_hint\"",
    "\"forbidden_external_resource\"",
    "\"forbidden_link_tag\"",
    "\"missing_stylesheet\"",
    "\"invalid_stylesheet_count\"",
    "\"forbidden_stylesheet_attribute\"",
    "fn validateServerCssPolicy",
    "\"forbidden_css_url\"",
    "fn validateServerSmokeTestPolicy",
    "\"invalid_smoke_selector\"",
    "fn validateServerJsPolicy",
    "\"forbidden_function_constructor\"",
    "\"forbidden_dynamic_import\"",
    "\"navigator.sendBeacon\"",
    "\"forbidden_sql_api\"",
    "\"cookieStore\"",
    "\"forbidden_appid_param\"",
    "fn manifestPermissionsContain",
    "fn validateServerNetworkPolicy",
    "\"invalid_network_origin\"",
    "fn hasUnknownRuntimeBridgeCall",
    "fn hasRuntimeBridgeCallMissingPermission",
    "fn hasRuntimeBridgeCallAppIdParam",
    "fn isAllowedRuntimeBridgeMethod",
    "\"unknown_method\"",
    "\"missing_permission\"",
    "\"NativeAIPlatformBridge\"",
  ];
  for (const snippet of required) {
    if (!source.includes(snippet)) {
      throw new Error(`server/src/main.zig missing ${snippet}`);
    }
  }
  return "bridge=core.step,runtime.capabilities,storage,mock-dialogs,notification,mock-network,app.log permissions=active-install budgets=bridge,network,logs,storage control=command,open,reset,logs,rollback,lifecycle,snapshot,migration,network-mocks,dialog-mocks,bridge-call,core-replay,assertions,timers,notifications,snapshot-compare,fault-injection install=migration-chain db=v0.4-schema,safe-token-file,auth-ban,backup-export-import rollback=data-version-guard production=control-disabled unsupported=platform_unsupported validate=package-policy,testids,methods,content-rating examples=static,json";
}

function checkNativeStatic() {
  const macBridge = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "WebBridge.swift"), "utf8");
  const macHost = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "WebHostView.swift"), "utf8");
  const macCore = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "ZigCoreBridge.swift"), "utf8");
  const macCoreShim = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "CZigCoreBridge", "CZigCoreBridge.c"), "utf8");
  const macPackage = fs.readFileSync(path.join(repoRoot, "native", "macos", "Package.swift"), "utf8");
  const macStorage = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "PlatformStorage.swift"), "utf8");
  const macNetwork = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "PlatformNetwork.swift"), "utf8");
  const macNotifications = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "PlatformNotifications.swift"), "utf8");
  const macDevControl = fs.readFileSync(path.join(repoRoot, "native", "macos", "Sources", "NativeAIHostMac", "DevControlPlane.swift"), "utf8");
  const iosBridge = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebBridge.swift"), "utf8");
  const iosHost = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "WebHostView.swift"), "utf8");
  const iosDialogs = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformDialogs.swift"), "utf8");
  const iosCore = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "ZigCoreBridge.swift"), "utf8");
  const iosCoreShim = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "CZigCoreBridge", "CZigCoreBridge.c"), "utf8");
  const iosPackage = fs.readFileSync(path.join(repoRoot, "native", "ios", "Package.swift"), "utf8");
  const iosStorage = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformStorage.swift"), "utf8");
  const iosNetwork = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformNetwork.swift"), "utf8");
  const iosNotifications = fs.readFileSync(path.join(repoRoot, "native", "ios", "Sources", "NativeAIHostIOS", "PlatformNotifications.swift"), "utf8");
  const windowsHost = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "WebViewHost.cpp"), "utf8");
  const windowsMain = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "main.cpp"), "utf8");
  const windowsBridge = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "WebBridge.cpp"), "utf8");
  const windowsDialogs = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformDialogs.cpp"), "utf8");
  const windowsNotifications = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformNotifications.cpp"), "utf8");
  const windowsDialogHeader = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformDialogs.h"), "utf8");
  const windowsCore = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "ZigCoreBridge.cpp"), "utf8");
  const windowsCoreHeader = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "ZigCoreBridge.h"), "utf8");
  const windowsStorage = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformStorage.cpp"), "utf8");
  const windowsNetwork = fs.readFileSync(path.join(repoRoot, "native", "windows", "src", "PlatformNetwork.cpp"), "utf8");
  const windowsCmake = fs.readFileSync(path.join(repoRoot, "native", "windows", "CMakeLists.txt"), "utf8");
  const windowsNativeBuildTest = fs.readFileSync(path.join(repoRoot, "tools", "fake-platform-host", "test", "windows-native-build.test.js"), "utf8");
  const linuxHost = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "webkit_host.c"), "utf8");
  const linuxBridge = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "web_bridge.c"), "utf8");
  const linuxDialogs = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_dialogs.c"), "utf8");
  const linuxNotifications = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_notifications.c"), "utf8");
  const linuxCore = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "zig_core_bridge.c"), "utf8");
  const linuxStorage = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_storage.c"), "utf8");
  const linuxNetwork = fs.readFileSync(path.join(repoRoot, "native", "linux", "src", "platform_network.c"), "utf8");
  const linuxMeson = fs.readFileSync(path.join(repoRoot, "native", "linux", "meson.build"), "utf8");
  const androidMain = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "MainActivity.kt"), "utf8");
  const androidBridge = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "NativeBridge.kt"), "utf8");
  const androidCore = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "ZigCoreBridge.kt"), "utf8");
  const androidCoreJni = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "cpp", "zig_core_jni.cpp"), "utf8");
  const androidCoreCmake = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "cpp", "CMakeLists.txt"), "utf8");
  const androidDatabase = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformDatabase.kt"), "utf8");
  const androidStorage = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformStorage.kt"), "utf8");
  const androidNetwork = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformNetwork.kt"), "utf8");
  const androidDialogs = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformDialogs.kt"), "utf8");
  const androidNotifications = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "src", "main", "java", "com", "nativeai", "platform", "PlatformNotifications.kt"), "utf8");
  const nativeNoAppIdParams = [
    [macBridge, 'request.params["appId"] != nil'],
    [iosBridge, 'request.params["appId"] != nil'],
    [androidBridge, 'request.params.has("appId")'],
    [windowsBridge, 'request.params.HasKey(L"appId")'],
    [linuxBridge, 'json_object_has_member(request.params, "appId")'],
  ];
  for (const [source, snippet] of nativeNoAppIdParams) {
    if (!source.includes(snippet) || !source.includes("Bridge params must not include appId; app id is channel-derived")) {
      throw new Error("native bridges must reject appId in bridge params using channel-derived context");
    }
  }
  const nativeNotificationValidators = [
    ["macOS", macNotifications, "validNotificationLevel(level)"],
    ["iOS", iosNotifications, "validNotificationLevel(level)"],
    ["Android", androidNotifications, 'level !in setOf("info", "success", "warning", "error")'],
    ["Windows", windowsNotifications, "ValidNotificationLevel(level)"],
    ["Linux", linuxNotifications, "valid_notification_level(level)"],
  ];
  for (const [label, source, levelSnippet] of nativeNotificationValidators) {
    for (const snippet of [
      "notification.toast requires message",
      "notification.toast level must be a string",
      "notification.toast level must be info, success, warning, or error",
      levelSnippet,
    ]) {
      if (!source.includes(snippet)) {
        throw new Error(`${label} notification.toast contract validation missing ${snippet}`);
      }
    }
  }
  const macRequired = [
    '"target": "macos"',
    '"appId": request.context.appId',
    '"devMode": nativeDevMode',
    '"limits":',
    '"storage.read": true',
    '"storage.write": true',
    '"network.request": true',
    '"core.step": core.isAvailable',
    "struct AppSandboxContext",
    "struct BridgeEnvelope",
    "hasOnlyRuntimeEnvelopeFields",
    "Runtime bridge envelope contains unknown top-level fields",
    "hasOnlyBridgeRequestFields",
    "Bridge request contains unknown top-level fields",
    "Bridge request id must be a non-empty string",
    "Bridge request timestamp must be a finite number",
    "Bridge request method must be a string",
    "Bridge request params must be an object",
    "message.frameInfo.isMainFrame",
    "mountToken",
    "networkPolicy",
    "denyPrivateNetwork",
    "permissionForBridgeMethod",
    "approvedPermissions.contains(permission)",
  ];
  for (const snippet of macRequired) {
    if (!macBridge.includes(snippet)) {
      throw new Error(`macOS runtime.capabilities missing ${snippet}`);
    }
  }
  for (const snippet of ["appRuntimeUserScript", "runtime.ready_for_port", "isGeneratedAppIndexURL", "htmlWithAppRuntimeBootstrap", "htmlWithAppRuntimeCSP", "script-src 'self' app-runtime:"]) {
    if (!macHost.includes(snippet)) {
      throw new Error(`macOS runtime scheme handler missing app-frame bootstrap: ${snippet}`);
    }
  }
  if (!macHost.includes('webapps/examples/\\(host)/')) {
    throw new Error("macOS runtime scheme handler must map app-runtime://{appId}/ paths to generated app packages");
  }
  for (const snippet of ["request.context.appId", "request.context.storagePrefix", "storagePrefixFailure"]) {
    if (!macStorage.includes(snippet)) {
      throw new Error(`macOS storage missing context enforcement: ${snippet}`);
    }
  }
  if (macStorage.includes("appId(for:")) {
    throw new Error("macOS storage must not derive app id from storage key");
  }
  for (const snippet of ["URLSessionConfiguration.ephemeral", "network_policy_denied", "NetworkPolicyRule", "willPerformHTTPRedirection", "isPrivateNetworkHost", "network.request private network targets are denied", "network.request credentials are not allowed"]) {
    if (!macNetwork.includes(snippet)) {
      throw new Error(`macOS network missing policy enforcement: ${snippet}`);
    }
  }
  for (const snippet of ["requestedTimeoutMs", "network.request timeoutMs must be a positive integer", "effectiveTimeoutMs", "NSURLErrorTimedOut", "timeoutFailure(id: request.id, timeoutMs: effectiveTimeoutMs)"]) {
    if (!macNetwork.includes(snippet)) {
      throw new Error(`macOS network missing timeoutMs parity: ${snippet}`);
    }
  }
  if (macNetwork.includes("TimeInterval(rule.timeoutMs) / 1000.0")) {
    throw new Error("macOS network.request must clamp request timeoutMs before configuring URLSession");
  }
  if (macNetwork.includes("platform_unsupported")) {
    throw new Error("macOS network.request must not remain a platform_unsupported stub");
  }
  if (macBridge.includes('"network.request": "native"') || macBridge.includes("pending-zig-link")) {
    throw new Error("macOS runtime.capabilities must use schema-shaped booleans");
  }
  for (const snippet of ['body["method"] as? String ?? ""', 'body["params"] as? [String: Any] ?? [:]']) {
    if (macBridge.includes(snippet)) {
      throw new Error(`macOS bridge must not keep lenient request parsing: ${snippet}`);
    }
  }
  for (const snippet of ['args["confirm"] as? Bool == true', "requires confirm: true"]) {
    if (!macDevControl.includes(snippet)) {
      throw new Error(`macOS dev control destructive reset missing ${snippet}`);
    }
  }
  for (const snippet of [
    "import CZigCoreBridge",
    "NATIVE_AI_ZIG_CORE_DYLIB",
    "RuntimeResourceLocator.repoRootURL()",
    "native_ai_zig_core_step_json",
    "native_ai_zig_core_free_output",
    "core.step app field does not match the channel-derived app id",
    "platform_unsupported",
  ]) {
    if (!macCore.includes(snippet)) {
      throw new Error(`macOS core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["mockedNetworkRequestTimeoutMs", "effectiveMockedNetworkTimeoutMs", "network.request timed out", '"delayMs": delayMs']) {
    if (!macDevControl.includes(snippet)) {
      throw new Error(`macOS dev-control network mocks missing fake-host timeout parity: ${snippet}`);
    }
  }
  for (const snippet of ["dlopen", "dlsym", "core_step_json", "core_free", "ZigCoreBuffer"]) {
    if (!macCoreShim.includes(snippet)) {
      throw new Error(`macOS C Zig core shim missing ${snippet}`);
    }
  }
  if (!macPackage.includes('.target(name: "CZigCoreBridge")') || !macPackage.includes('dependencies: ["CZigCoreBridge"]')) {
    throw new Error("macOS package must include the C Zig core bridge target");
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
    [iosHost, "appRuntimeUserScript"],
    [iosHost, "runtime.ready_for_port"],
    [iosHost, "isGeneratedAppIndexURL"],
    [iosHost, "htmlWithAppRuntimeBootstrap"],
    [iosHost, "htmlWithAppRuntimeCSP"],
    [iosHost, "script-src 'self' app-runtime:"],
    [iosHost, 'webapps/examples/\\(host)/'],
    [iosHost, "setDialogPresenterProvider"],
    [iosHost, "presentingViewController(from:"],
    [iosBridge, '"target": "ios-simulator"'],
    [iosBridge, '"appId": request.context.appId'],
    [iosBridge, '"devMode": nativeDevMode'],
    [iosBridge, '"limits":'],
    [iosBridge, '"storage.read": true'],
    [iosBridge, '"storage.write": true'],
    [iosBridge, '"network.request": true'],
    [iosBridge, '"dialog.openFile": true'],
    [iosBridge, '"dialog.saveFile": true'],
    [iosBridge, '"core.step": core.isAvailable'],
    [iosBridge, "typealias BridgeReply"],
    [iosBridge, "dispatch(request) { [weak self] response in"],
    [iosBridge, "dialogs.openFile(request, reply: reply)"],
    [iosBridge, "struct BridgeEnvelope"],
    [iosBridge, "hasOnlyRuntimeEnvelopeFields"],
    [iosBridge, "Runtime bridge envelope contains unknown top-level fields"],
    [iosBridge, "hasOnlyBridgeRequestFields"],
    [iosBridge, "Bridge request contains unknown top-level fields"],
    [iosBridge, "Bridge request id must be a non-empty string"],
    [iosBridge, "Bridge request timestamp must be a finite number"],
    [iosBridge, "Bridge request method must be a string"],
    [iosBridge, "Bridge request params must be an object"],
    [iosBridge, "message.frameInfo.isMainFrame"],
    [iosBridge, "mountToken"],
    [iosBridge, "struct AppSandboxContext"],
    [iosBridge, "networkPolicy"],
    [iosBridge, "denyPrivateNetwork"],
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
  for (const snippet of ['body["method"] as? String ?? ""', 'body["params"] as? [String: Any] ?? [:]']) {
    if (iosBridge.includes(snippet)) {
      throw new Error(`iOS bridge must not keep lenient request parsing: ${snippet}`);
    }
  }
  for (const snippet of ["URLSessionConfiguration.ephemeral", "network_policy_denied", "NetworkPolicyRule", "willPerformHTTPRedirection", "isPrivateNetworkHost", "network.request private network targets are denied", "network.request credentials are not allowed"]) {
    if (!iosNetwork.includes(snippet)) {
      throw new Error(`iOS network missing policy enforcement: ${snippet}`);
    }
  }
  for (const snippet of ["requestedTimeoutMs", "network.request timeoutMs must be a positive integer", "effectiveTimeoutMs", "NSURLErrorTimedOut", "timeoutFailure(id: request.id, timeoutMs: effectiveTimeoutMs)"]) {
    if (!iosNetwork.includes(snippet)) {
      throw new Error(`iOS network missing timeoutMs parity: ${snippet}`);
    }
  }
  if (iosNetwork.includes("TimeInterval(rule.timeoutMs) / 1000.0")) {
    throw new Error("iOS network.request must clamp request timeoutMs before configuring URLSession");
  }
  if (iosNetwork.includes("platform_unsupported")) {
    throw new Error("iOS network.request must not remain a platform_unsupported stub");
  }
  for (const snippet of ["UIDocumentPickerViewController", "forOpeningContentTypes", "forExporting", "UIDocumentPickerDelegate", "startAccessingSecurityScopedResource", "dialog_cancelled"]) {
    if (!iosDialogs.includes(snippet)) {
      throw new Error(`iOS dialogs missing ${snippet}`);
    }
  }
  if (iosDialogs.includes("is not available in the iOS host yet") || iosBridge.includes('"dialog.openFile": false')) {
    throw new Error("iOS dialogs must not remain placeholder stubs or disabled capabilities");
  }
  for (const snippet of [
    "import CZigCoreBridge",
    "Library(linked: true)",
    "NATIVE_AI_ZIG_CORE_DYLIB",
    "RuntimeResourceLocator.repoRootURL()",
    "native_ai_zig_core_step_json",
    "native_ai_zig_core_free_output",
    "core.step app field does not match the channel-derived app id",
    "platform_unsupported",
  ]) {
    if (!iosCore.includes(snippet)) {
      throw new Error(`iOS core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["RTLD_DEFAULT", "dlopen", "dlsym", "core_step_json", "core_free", "ZigCoreBuffer"]) {
    if (!iosCoreShim.includes(snippet)) {
      throw new Error(`iOS C Zig core shim missing ${snippet}`);
    }
  }
  if (!iosPackage.includes('.target(name: "CZigCoreBridge")') || !iosPackage.includes('dependencies: ["CZigCoreBridge"]')) {
    throw new Error("iOS package must include the C Zig core bridge target");
  }
  const windowsRequired = [
    [windowsHost, "SetVirtualHostNameToFolderMapping"],
    [windowsHost, "add_WebMessageReceived"],
    [windowsHost, "get_Source"],
    [windowsHost, "https://runtime.local.platform/"],
    [windowsHost, "SandboxContextForApp"],
    [windowsHost, "IsRuntimeEnvelope"],
    [windowsHost, "HasValidRuntimeEnvelope"],
    [windowsHost, "HasOnlyRuntimeEnvelopeFields"],
    [windowsHost, "Runtime bridge envelope is required"],
    [windowsHost, "mountToken"],
    [windowsHost, "IsKnownExampleAppId"],
    [windowsHost, "get_BrowserVersionString"],
    [windowsHost, "WebView2RuntimeMeetsMinimum"],
    [windowsHost, "WebView2 runtime version 1.0.2592 or later is required"],
    [windowsHost, "RunRuntimeLoadSmoke"],
    [windowsHost, "StorageNotesResponseContainsSmokeValue"],
    [windowsHost, "windows_smoke_runtime_app_seed_storage"],
    [windowsBridge, "HasOnlyBridgeRequestFields"],
    [windowsBridge, "Bridge request contains unknown top-level fields"],
    [windowsBridge, "Bridge request timestamp must be a finite number"],
    [windowsBridge, "Bridge request id must be a non-empty string"],
    [windowsBridge, "Bridge request method must be a string"],
    [windowsBridge, "Bridge request params must be an object"],
    [windowsBridge, "permissionForBridgeMethod"],
    [windowsBridge, "approvedPermissions.contains(permission"],
    [windowsBridge, 'result.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId))'],
    [windowsBridge, 'result.Insert(L"devMode", json::JsonValue::CreateBooleanValue(NativeDevMode()))'],
    [windowsBridge, 'features.Insert(L"storage.read", json::JsonValue::CreateBooleanValue(true))'],
    [windowsBridge, 'features.Insert(L"storage.write", json::JsonValue::CreateBooleanValue(true))'],
    [windowsBridge, 'features.Insert(L"network.request", json::JsonValue::CreateBooleanValue(true))'],
    [windowsHost, "NetworkPolicyForApp"],
    [windowsHost, "DenyPrivateNetworkForApp"],
    [windowsHost, ".networkPolicy"],
    [windowsHost, ".denyPrivateNetwork"],
    [windowsHost, "std::make_unique<WebBridge>(DatabasePath(), window)"],
    [windowsBridge, 'features.Insert(L"dialog.openFile", json::JsonValue::CreateBooleanValue(true))'],
    [windowsBridge, 'features.Insert(L"dialog.saveFile", json::JsonValue::CreateBooleanValue(true))'],
    [windowsBridge, 'features.Insert(L"core.step", json::JsonValue::CreateBooleanValue(core_.IsAvailable()))'],
    [windowsStorage, "request.context.appId"],
    [windowsStorage, "request.context.storagePrefix"],
    [windowsStorage, "storagePrefixFailure"],
    [windowsNetwork, "RequestedTimeoutMs"],
    [windowsNetwork, "network.request timeoutMs must be a positive integer"],
    [windowsNetwork, "EffectiveTimeoutMs"],
    [windowsNetwork, "ERROR_WINHTTP_TIMEOUT"],
    [windowsNetwork, 'L"timeout"'],
  ];
  for (const [source, snippet] of windowsRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Windows host missing ${snippet}`);
    }
  }
  if (windowsHost.includes("response = bridge_->HandleJson(body, SandboxContextFromSource(sourceText))") ||
      windowsHost.includes("SandboxContextFromSource")) {
    throw new Error("Windows WebView2 bridge dispatch must require runtime envelopes");
  }
  if (windowsStorage.includes("appIdFor")) {
    throw new Error("Windows storage must not derive app id from storage key");
  }
  for (const snippet of ["WinHttpOpenRequest", "network_policy_denied", "NetworkPolicyRule", "WINHTTP_DISABLE_REDIRECTS", "IsPrivateNetworkHost", "network.request private network targets are denied", "network.request credentials are not allowed"]) {
    if (!windowsNetwork.includes(snippet)) {
      throw new Error(`Windows network missing policy enforcement: ${snippet}`);
    }
  }
  if (windowsNetwork.includes("platform_unsupported")) {
    throw new Error("Windows network.request must not remain a platform_unsupported stub");
  }
  for (const snippet of ["IFileOpenDialog", "IFileSaveDialog", "FOS_FORCEFILESYSTEM", "dialog_cancelled", "ReadTextFile", "WriteTextFile"]) {
    if (!windowsDialogs.includes(snippet)) {
      throw new Error(`Windows dialogs missing ${snippet}`);
    }
  }
  if (windowsDialogs.includes("will be wired")) {
    throw new Error("Windows dialogs must not remain placeholder stubs");
  }
  if (!windowsDialogHeader.includes("explicit PlatformDialogs(HWND ownerWindow")) {
    throw new Error("Windows dialogs must accept an owner HWND");
  }
  for (const snippet of ["LoadLibraryW", "GetProcAddress", "core_step_json", "core_free", "NATIVE_AI_ZIG_CORE_DLL", 'exeDir / L"zig_core.dll"', "core.step app field does not match the channel-derived app id"]) {
    if (!windowsCore.includes(snippet)) {
      throw new Error(`Windows Zig core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["bool IsAvailable() const", "CoreStepJsonFn", "CoreFreeFn"]) {
    if (!windowsCoreHeader.includes(snippet)) {
      throw new Error(`Windows Zig core bridge header missing ${snippet}`);
    }
  }
  for (const snippet of ["winhttp", "ole32", "NATIVE_AI_ZIG_CORE_DLL", "copy_if_different"]) {
    if (!windowsCmake.includes(snippet)) {
      throw new Error(`Windows native bridge must link ${snippet}`);
    }
  }
  for (const snippet of [
    "RecordProductionGuardAudit",
    "IsForbiddenDevFlag",
    "--allow-unsigned-dev",
    "--allow-runtime-mismatch",
    "--control-plane-port",
    "native.production_guard",
    "dev_only_flag",
  ]) {
    if (!windowsMain.includes(snippet)) {
      throw new Error(`Windows production guard missing ${snippet}`);
    }
  }
  for (const snippet of [
    "Windows release host rejects dev-only startup flags and audits the rejection",
    "--config\", \"Release",
    "--allow-runtime-mismatch=1",
    "--control-plane-port=5123",
    "native\\.production_guard",
    "platform.sqlite",
  ]) {
    if (!windowsNativeBuildTest.includes(snippet)) {
      throw new Error(`Windows native build test missing production guard coverage: ${snippet}`);
    }
  }
  const linuxRequired = [
    [linuxHost, "webkit_security_manager_register_uri_scheme_as_secure"],
    [linuxHost, "webkit_user_content_manager_register_script_message_handler_with_reply"],
    [linuxHost, "script-message-with-reply-received::NativeAIPlatformBridge"],
    [linuxHost, "jsc_value_to_json"],
    [linuxHost, "webkit_script_message_reply_return_value"],
    [linuxHost, "logical_path_for_runtime_uri"],
    [linuxHost, "logical_path_is_generated_app_index"],
    [linuxHost, "html_with_app_runtime_bootstrap"],
    [linuxHost, "html_with_app_runtime_csp"],
    [linuxHost, "script-src 'self' app-runtime:"],
    [linuxHost, "g_memory_input_stream_new_from_data"],
    [linuxHost, "runtime-web"],
    [linuxHost, "content_type_for_path"],
    [linuxHost, "app-runtime://runtime/index.html"],
    [linuxHost, "sandbox_context_from_uri"],
    [linuxHost, "sandbox_context_for_app"],
    [linuxHost, "is_runtime_envelope"],
    [linuxHost, "has_valid_runtime_envelope"],
    [linuxHost, "has_only_runtime_envelope_fields"],
    [linuxHost, "Runtime bridge envelope is required"],
    [linuxHost, "is_known_example_app_id"],
    [linuxHost, "mount_token"],
    [linuxHost, "network_policy_for_app"],
    [linuxHost, "deny_private_network_for_app"],
    [linuxHost, ".network_policy"],
    [linuxHost, ".deny_private_network"],
    [linuxHost, "web_bridge_new(db_path, GTK_WINDOW(host->window))"],
    [linuxBridge, "permission_for_bridge_method"],
    [linuxBridge, "approved_permissions_contains"],
    [linuxBridge, "has_only_bridge_request_fields"],
    [linuxBridge, "Bridge request contains unknown top-level fields"],
    [linuxBridge, "Bridge request timestamp must be a finite number"],
    [linuxBridge, "Bridge request id must be a non-empty string"],
    [linuxBridge, "Bridge request method must be a string"],
    [linuxBridge, "Bridge request params must be an object"],
    [linuxBridge, 'json_builder_set_member_name(builder, "appId")'],
    [linuxBridge, "request->context.app_id"],
    [linuxBridge, "native_dev_mode()"],
    [linuxBridge, '"storage.read"'],
    [linuxBridge, '"storage.write"'],
    [linuxBridge, '"network.request"'],
    [linuxBridge, '"dialog.openFile"'],
    [linuxBridge, "platform_dialogs_init(&bridge->dialogs, owner_window)"],
    [linuxBridge, "zig_core_bridge_init"],
    [linuxBridge, "zig_core_bridge_is_available(&bridge->core)"],
    [linuxStorage, "request->context.app_id"],
    [linuxStorage, "request->context.storage_prefix"],
    [linuxStorage, "storage_prefix_failure"],
    [linuxNetwork, "requested_timeout_ms"],
    [linuxNetwork, "network.request timeoutMs must be a positive integer"],
    [linuxNetwork, "effective_timeout_ms"],
    [linuxNetwork, "RequestTimeout"],
    [linuxNetwork, "g_cond_wait_until"],
    [linuxNetwork, "g_cancellable_cancel"],
    [linuxNetwork, "G_IO_ERROR_TIMED_OUT"],
    [linuxNetwork, "G_IO_ERROR_CANCELLED"],
    [linuxNetwork, '"timeout"'],
  ];
  for (const [source, snippet] of linuxRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Linux host missing ${snippet}`);
    }
  }
  if (linuxHost.includes("var handler=window.webkit") ||
      linuxHost.includes("envelope={appId:appId")) {
    throw new Error("Linux native host must not inject a direct AppRuntime/native bridge into generated app frames");
  }
  if (linuxHost.includes("response = web_bridge_handle_json(host->bridge, payload")) {
    throw new Error("Linux WebKit message handler must require runtime-owned envelopes");
  }
  if (linuxStorage.includes("app_id_for_key")) {
    throw new Error("Linux storage must not derive app id from storage key");
  }
  for (const snippet of ['g_object_set(session, "timeout"', "(timeout_ms + 999) / 1000"]) {
    if (linuxNetwork.includes(snippet)) {
      throw new Error(`Linux network.request must enforce timeoutMs with millisecond cancellable precision, not SoupSession seconds: ${snippet}`);
    }
  }
  for (const snippet of ["soup_session_send_and_read", "network_policy_denied", "NetworkPolicyRule", "SOUP_MESSAGE_NO_REDIRECT", "is_private_network_host", "network.request private network targets are denied", "network.request credentials are not allowed"]) {
    if (!linuxNetwork.includes(snippet)) {
      throw new Error(`Linux network missing policy enforcement: ${snippet}`);
    }
  }
  if (linuxNetwork.includes("platform_unsupported")) {
    throw new Error("Linux network.request must not remain a platform_unsupported stub");
  }
  for (const snippet of ["GtkFileChooserNative", "gtk_native_dialog_show", "g_main_loop_run", "gtk_file_chooser_get_file", "g_file_get_contents", "g_file_set_contents", "dialog_cancelled"]) {
    if (!linuxDialogs.includes(snippet)) {
      throw new Error(`Linux dialogs missing ${snippet}`);
    }
  }
  if (linuxDialogs.includes("will be wired") || linuxBridge.includes('json_builder_add_boolean_value(builder, FALSE);')) {
    throw new Error("Linux dialogs must not remain placeholder stubs or disabled capabilities");
  }
  for (const snippet of ["dlopen", "dlsym", "core_step_json", "core_free", "NATIVE_AI_ZIG_CORE_SO", "core.step app field does not match the channel-derived app id"]) {
    if (!linuxCore.includes(snippet)) {
      throw new Error(`Linux Zig core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["libsoup-3.0", "find_library('dl'", "dl_dep"]) {
    if (!linuxMeson.includes(snippet)) {
      throw new Error(`Linux native bridge missing Meson dependency ${snippet}`);
    }
  }
  const androidRequired = [
    [androidMain, "WebViewCompat.addWebMessageListener"],
    [androidMain, "https://appassets.androidplatform.net"],
    [androidMain, "allowFileAccess = false"],
    [androidMain, "ComponentActivity"],
    [androidMain, "AssetRootPathHandler"],
    [androidMain, "sourceOrigin.toString()"],
    [androidMain, "replyProxy.postMessage(response)"],
    [androidMain, "PlatformDialogs(this)"],
    [androidMain, "sandboxContextFromManifest"],
    [androidMain, "exampleAppIds.contains(appId)"],
    [androidMain, "NetworkPolicyRule.fromManifest"],
    [androidMain, "denyPrivateNetwork"],
    [androidMain, "webapps/examples/$appId/manifest.json"],
    [androidMain, 'webView.loadUrl("https://appassets.androidplatform.net/runtime/index.html")'],
    [androidBridge, "fun handleEnvelope"],
    [androidBridge, "isMainFrame"],
    [androidBridge, "trustedRuntimeOrigin"],
    [androidBridge, "hasOnlyRuntimeEnvelopeFields"],
    [androidBridge, "Runtime bridge envelope contains unknown top-level fields"],
    [androidBridge, "hasOnlyBridgeRequestFields"],
    [androidBridge, "Bridge request contains unknown top-level fields"],
    [androidBridge, "Bridge request id must be a non-empty string"],
    [androidBridge, "Bridge request timestamp must be a finite number"],
    [androidBridge, "Bridge request method must be a string"],
    [androidBridge, "Bridge request params must be an object"],
    [androidBridge, "mountToken"],
    [androidBridge, "permissionForBridgeMethod"],
    [androidBridge, "approvedPermissions.contains(permission)"],
    [androidBridge, "respond: (String) -> Unit"],
    [androidBridge, 'dialogs.openFile(request) { response -> respondWithLog(response) }'],
    [androidBridge, '"runtime_sessions"'],
    [androidBridge, '"bridge_calls"'],
    [androidBridge, '"core_events"'],
    [androidBridge, '"core_actions"'],
    [androidBridge, '"appId" to request.context.appId'],
    [androidBridge, '"devMode" to BuildConfig.DEBUG'],
    [androidBridge, '"storage.read" to true'],
    [androidBridge, '"storage.write" to true'],
    [androidBridge, '"network.request" to true'],
    [androidBridge, '"dialog.openFile" to true'],
    [androidBridge, '"dialog.saveFile" to true'],
    [androidBridge, '"core.step" to core.isAvailable()'],
    [androidBridge, "networkPolicy"],
    [androidBridge, "denyPrivateNetwork"],
    [androidDatabase, "SQLiteOpenHelper"],
    [androidDatabase, "PRAGMA integrity_check"],
    [androidDatabase, 'assets.list("db/sqlite")'],
    [androidStorage, "PlatformDatabase(context)"],
    [androidStorage, "request.context.appId"],
    [androidStorage, "request.context.storagePrefix"],
  ];
  for (const [source, snippet] of androidRequired) {
    if (!source.includes(snippet)) {
      throw new Error(`Android host missing ${snippet}`);
    }
  }
  for (const snippet of ['body.optString("method")', 'body.optJSONObject("params") ?: JSONObject()']) {
    if (androidBridge.includes(snippet)) {
      throw new Error(`Android bridge must not keep lenient request parsing: ${snippet}`);
    }
  }
  const androidGradle = fs.readFileSync(path.join(repoRoot, "native", "android", "app", "build.gradle.kts"), "utf8");
  for (const snippet of ["syncNativeAiAssets", 'into("runtime")', 'into("webapps")', 'into("db/sqlite")', "buildConfig = true", "assets.srcDir(generatedNativeAiAssets)", "externalNativeBuild", 'path = file("src/main/cpp/CMakeLists.txt")', "androidx.activity:activity-ktx"]) {
    if (!androidGradle.includes(snippet)) {
      throw new Error(`Android Gradle asset sync missing ${snippet}`);
    }
  }
  for (const snippet of ["ActivityResultContracts.OpenDocument", "ActivityResultContracts.OpenMultipleDocuments", "ActivityResultContracts.CreateDocument", "openDocument.launch", "openDocuments.launch", "createDocument.launch", "contentResolver.openInputStream", "contentResolver.openOutputStream", "dialog_cancelled"]) {
    if (!androidDialogs.includes(snippet)) {
      throw new Error(`Android dialogs missing ${snippet}`);
    }
  }
  if (androidDialogs.includes("is not implemented on Android yet") || androidBridge.includes('"dialog.openFile" to false')) {
    throw new Error("Android dialogs must not remain placeholder stubs or disabled capabilities");
  }
  for (const snippet of ["HttpURLConnection", "network_policy_denied", "NetworkPolicyRule", "instanceFollowRedirects = false", "CountDownLatch", "isPrivateNetworkHost", "network.request private network targets are denied", "network.request credentials are not allowed"]) {
    if (!androidNetwork.includes(snippet)) {
      throw new Error(`Android network missing policy enforcement: ${snippet}`);
    }
  }
  for (const snippet of ["requestedTimeoutMs", "network.request timeoutMs must be a positive integer", "effectiveTimeoutMs", "timeoutFailure(request, effectiveTimeoutMs)", "JSONObject(mapOf(\"timeoutMs\" to timeoutMs))"]) {
    if (!androidNetwork.includes(snippet)) {
      throw new Error(`Android network missing timeoutMs parity: ${snippet}`);
    }
  }
  for (const snippet of ["rule.timeoutMs + 1_000", "connectTimeout = rule.timeoutMs", "readTimeout = rule.timeoutMs"]) {
    if (androidNetwork.includes(snippet)) {
      throw new Error(`Android network.request must clamp request timeoutMs before transport timeout: ${snippet}`);
    }
  }
  if (androidNetwork.includes("platform_unsupported")) {
    throw new Error("Android network.request must not remain a platform_unsupported stub");
  }
  for (const snippet of ["System.loadLibrary(\"zig_core_jni\")", "external fun nativeStep", "core.step app field does not match the channel-derived app id", "JSONObject(request.params.toString())"]) {
    if (!androidCore.includes(snippet)) {
      throw new Error(`Android Zig core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["dlopen(\"libzig_core.so\"", "dlsym", "core_step_json", "core_free", "JNI_OnUnload"]) {
    if (!androidCoreJni.includes(snippet)) {
      throw new Error(`Android JNI Zig core bridge missing ${snippet}`);
    }
  }
  for (const snippet of ["add_library(zig_core_jni SHARED zig_core_jni.cpp)", "target_link_libraries(zig_core_jni PRIVATE", "dl"]) {
    if (!androidCoreCmake.includes(snippet)) {
      throw new Error(`Android CMake Zig core bridge missing ${snippet}`);
    }
  }
  return "macos.capabilities=schema-shaped core=zig-dylib storage=context-enforced ios.webbridge=context-enforced dialogs=document-picker core=linked-or-dylib windows.webview2=origin-checked dialogs=common-dialogs core=zig-dll linux.webkit=scheme-checked dialogs=gtk-native core=zig-so android.webmessage=origin-checked dialogs=activity-result core=jni-so";
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function jsonFiles(root) {
  return walk(root).filter((filePath) => filePath.endsWith(".json"));
}

function isExamplePath(filePath) {
  const rel = relative(filePath);
  return rel.startsWith("webapps/examples/");
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
