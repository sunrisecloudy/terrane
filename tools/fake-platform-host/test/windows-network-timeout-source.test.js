import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Windows network.request honors request timeoutMs and maps WinHTTP timeouts", () => {
  const source = fs.readFileSync(path.join(repoRoot, "native/windows/src/PlatformNetwork.cpp"), "utf8");

  assert.match(source, /RequestedTimeoutMs/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /EffectiveTimeoutMs/);
  assert.match(source, /std::min\(rule\.timeoutMs, requestedTimeout\.value\(\)\)/);
  assert.match(source, /ERROR_WINHTTP_TIMEOUT/);
  assert.match(source, /L"timeout"/);
  assert.match(source, /network\.request timed out/);
  assert.doesNotMatch(source, /auto timeout = static_cast<int>\(rule->timeoutMs\)/);
});
