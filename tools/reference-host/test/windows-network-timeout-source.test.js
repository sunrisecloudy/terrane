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
  assert.match(source, /RequestTimeout/);
  assert.match(source, /G_TIME_SPAN_MILLISECOND/);
  assert.match(source, /g_cond_wait_until/);
  assert.match(source, /g_cancellable_cancel/);
  assert.match(source, /soup_session_send_and_read\(session, message, cancellable, &error\)/);
  assert.match(source, /G_IO_ERROR_TIMED_OUT/);
  assert.match(source, /G_IO_ERROR_CANCELLED/);
  assert.match(source, /"timeout"/);
  assert.match(source, /network\.request timed out/);
  assert.doesNotMatch(source, /g_object_set\(session,\s*"timeout"/);
  assert.doesNotMatch(source, /\(timeout_ms \+ 999\) \/ 1000/);
  assert.doesNotMatch(source, /rule->timeout_ms \/ 1000/);
});

test("macOS network.request honors request timeoutMs and maps URLSession timeouts", () => {
  const source = read("native/macos/Sources/TerraneHostMac/PlatformNetwork.swift");

  assert.match(source, /requestedTimeoutMs/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /effectiveTimeoutMs/);
  assert.match(source, /min\(rule\.timeoutMs, \$0\)/);
  assert.match(source, /NSURLErrorTimedOut/);
  assert.match(source, /timeoutFailure\(id: request\.id, timeoutMs: effectiveTimeoutMs\)/);
  assert.match(source, /details: \["timeoutMs": timeoutMs\]/);
  assert.doesNotMatch(source, /TimeInterval\(rule\.timeoutMs\) \/ 1000\.0/);
});

test("iOS network.request honors request timeoutMs and maps URLSession timeouts", () => {
  const source = read("native/ios/Sources/TerraneHostIOS/PlatformNetwork.swift");

  assert.match(source, /requestedTimeoutMs/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /effectiveTimeoutMs/);
  assert.match(source, /min\(rule\.timeoutMs, \$0\)/);
  assert.match(source, /NSURLErrorTimedOut/);
  assert.match(source, /timeoutFailure\(id: request\.id, timeoutMs: effectiveTimeoutMs\)/);
  assert.match(source, /details: \["timeoutMs": timeoutMs\]/);
  assert.doesNotMatch(source, /TimeInterval\(rule\.timeoutMs\) \/ 1000\.0/);
});

test("Android network.request honors request timeoutMs and maps socket timeouts", () => {
  const source = read("native/android/app/src/main/java/com/terrane/platform/PlatformNetwork.kt");

  assert.match(source, /requestedTimeoutMs/);
  assert.match(source, /OkHttpClient\.Builder/);
  assert.match(source, /followRedirects\(false\)/);
  assert.match(source, /network\.request timeoutMs must be a positive integer/);
  assert.match(source, /effectiveTimeoutMs/);
  assert.match(source, /minOf\(rule\.timeoutMs, it\)/);
  assert.match(source, /callTimeout\(effectiveTimeoutMs\.toLong\(\), TimeUnit\.MILLISECONDS\)/);
  assert.match(source, /SocketTimeoutException/);
  assert.match(source, /timeoutFailure\(request, effectiveTimeoutMs\)/);
  assert.match(source, /JSONObject\(mapOf\("timeoutMs" to timeoutMs\)\)/);
  assert.doesNotMatch(source, /HttpURLConnection/);
  assert.doesNotMatch(source, /rule\.timeoutMs \+ 1_000/);
  assert.doesNotMatch(source, /connectTimeout = rule\.timeoutMs/);
  assert.doesNotMatch(source, /readTimeout = rule\.timeoutMs/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
