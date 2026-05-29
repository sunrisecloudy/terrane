import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Windows and Linux native bridges persist bridge call logs", () => {
  const windowsBridge = read("native/windows/src/WebBridge.cpp");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxBridge = read("native/linux/src/web_bridge.c");
  const linuxHost = read("native/linux/src/webkit_host.c");

  assert.match(windowsBridge, /INSERT INTO runtime_sessions/);
  assert.match(windowsBridge, /INSERT INTO bridge_calls/);
  assert.match(windowsBridge, /params_json/);
  assert.match(windowsBridge, /result_json/);
  assert.match(windowsBridge, /error_json/);
  assert.match(windowsHost, /SELECT COUNT\(\*\) FROM bridge_calls WHERE app_id = \? AND method = \?/);
  assert.match(windowsHost, /fixed bridge surface smoke did not persist bridge_calls rows/);

  assert.match(linuxBridge, /INSERT INTO runtime_sessions/);
  assert.match(linuxBridge, /INSERT INTO bridge_calls/);
  assert.match(linuxBridge, /params_json/);
  assert.match(linuxBridge, /result_json/);
  assert.match(linuxBridge, /error_json/);
  assert.match(linuxHost, /SELECT COUNT\(\*\) FROM bridge_calls WHERE app_id = \? AND method = \?/);
  assert.match(linuxHost, /fixed bridge surface smoke did not persist bridge_calls rows/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
