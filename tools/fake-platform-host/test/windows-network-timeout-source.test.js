import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Windows network.request honors request timeoutMs and maps WinHTTP timeouts", () => {
  const source = read("native/windows/src/PlatformNetwork.cpp");

  assert.match(source, /RequestedTimeoutMs/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /EffectiveTimeoutMs/);
  assert.match(source, /std::min\(rule\.timeoutMs, requestedTimeout\.value\(\)\)/);
  assert.match(source, /ERROR_WINHTTP_TIMEOUT/);
  assert.match(source, /L"timeout"/);
  assert.match(source, /network\.request timed out/);
  assert.doesNotMatch(source, /auto timeout = static_cast<int>\(rule->timeoutMs\)/);
});

test("Linux network.request honors request timeoutMs and maps GLib timeouts", () => {
  const source = read("native/linux/src/platform_network.c");

  assert.match(source, /requested_timeout_ms/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /effective_timeout_ms/);
  assert.match(source, /MIN\(rule->timeout_ms, requested_timeout\)/);
  assert.match(source, /G_IO_ERROR_TIMED_OUT/);
  assert.match(source, /"timeout"/);
  assert.match(source, /network\.request timed out/);
  assert.doesNotMatch(source, /rule->timeout_ms \/ 1000/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
