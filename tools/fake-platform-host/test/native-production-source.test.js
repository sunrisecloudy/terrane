import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("macOS production guard rejects dev-only startup flags outside DEBUG", () => {
  const app = read("native/macos/Sources/NativeAIHostMac/App.swift");
  const tests = read("native/macos/Tests/NativeAIHostMacTests/NativeHostTests.swift");

  assert.match(app, /NativeProductionGuard\.rejectDevOnlyFlagsIfNeeded\(\)/);
  assert.match(app, /"--control-plane-port"/);
  assert.match(app, /"--allow-runtime-mismatch"/);
  assert.match(app, /"--allow-unsigned-dev"/);
  assert.equal(app.includes('argument == flag || argument.hasPrefix("\\(flag)=")'), true);
  assert.match(app, /INSERT INTO control_commands/);
  assert.match(app, /native\.production_guard/);
  assert.match(app, /#if DEBUG\s+true\s+#else\s+false\s+#endif/s);
  assert.match(tests, /productionGuardRejectsExactDevOnlyStartupFlags/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
