import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("desktop production guards reject dev-only startup flags outside debug builds", () => {
  const app = read("native/macos/Sources/NativeAIHostMac/App.swift");
  const tests = read("native/macos/Tests/NativeAIHostMacTests/NativeHostTests.swift");
  const windowsMain = read("native/windows/src/main.cpp");
  const windowsDatabase = read("native/windows/src/PlatformDatabase.cpp");
  const linuxMain = read("native/linux/src/main.c");
  const linuxDatabase = read("native/linux/src/platform_database.c");
  const linuxMeson = read("native/linux/meson.build");

  assert.match(app, /NativeProductionGuard\.rejectDevOnlyFlagsIfNeeded\(\)/);
  assert.match(app, /"--control-plane-port"/);
  assert.match(app, /"--allow-runtime-mismatch"/);
  assert.match(app, /"--allow-unsigned-dev"/);
  assert.equal(app.includes('argument == flag || argument.hasPrefix("\\(flag)=")'), true);
  assert.match(app, /INSERT INTO control_commands/);
  assert.match(app, /native\.production_guard/);
  assert.match(app, /#if DEBUG\s+true\s+#else\s+false\s+#endif/s);
  assert.match(tests, /productionGuardRejectsExactDevOnlyStartupFlags/);

  assert.match(windowsMain, /RejectDevOnlyFlagsIfNeeded\(\)/);
  assert.match(windowsMain, /L"--control-plane-port"/);
  assert.match(windowsMain, /L"--allow-runtime-mismatch"/);
  assert.match(windowsMain, /L"--allow-unsigned-dev"/);
  assert.match(windowsMain, /#ifdef _DEBUG\s+return true;\s+#else\s+return false;\s+#endif/s);
  assert.match(windowsMain, /argument\[flag\.size\(\)\] == L'='/);
  assert.match(windowsMain, /PlatformDatabase database\(ProductionGuardDatabasePath\(\)\)/);
  assert.match(windowsMain, /INSERT OR REPLACE INTO control_sessions/);
  assert.match(windowsMain, /INSERT INTO control_commands/);
  assert.match(windowsMain, /native\.production_guard/);
  assert.match(windowsMain, /dev_only_flag/);
  assert.match(windowsDatabase, /CREATE TABLE IF NOT EXISTS control_sessions/);
  assert.match(windowsDatabase, /CREATE TABLE IF NOT EXISTS control_commands/);

  assert.match(linuxMain, /native_ai_reject_dev_only_flags_if_needed\(argc, argv\)/);
  assert.match(linuxMain, /native_ai_application_argv_without_dev_flags\(argc, argv, &application_argc\)/);
  assert.match(linuxMain, /"--control-plane-port"/);
  assert.match(linuxMain, /"--allow-runtime-mismatch"/);
  assert.match(linuxMain, /"--allow-unsigned-dev"/);
  assert.match(linuxMain, /#ifndef NDEBUG\s+return TRUE;\s+#else\s+return FALSE;\s+#endif/s);
  assert.match(linuxMain, /argument\[flag_length\] == '='/);
  assert.match(linuxMain, /platform_database_open\(db_path\)/);
  assert.match(linuxMain, /INSERT OR REPLACE INTO control_sessions/);
  assert.match(linuxMain, /INSERT INTO control_commands/);
  assert.match(linuxMain, /native\.production_guard/);
  assert.match(linuxMain, /dev_only_flag/);
  assert.match(linuxDatabase, /CREATE TABLE IF NOT EXISTS control_sessions/);
  assert.match(linuxDatabase, /CREATE TABLE IF NOT EXISTS control_commands/);
  assert.match(linuxMeson, /'b_ndebug=if-release'/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
