import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Apple native network policy enforces allow pathPrefix", () => {
  for (const relativePath of [
    "native/macos/Sources/NativeAIHostMac/PlatformNetwork.swift",
    "native/ios/Sources/NativeAIHostIOS/PlatformNetwork.swift",
  ]) {
    const source = read(relativePath);
    assert.match(source, /let pathPrefix: String\?/);
    assert.match(source, /raw\["pathPrefix"\] as\? String/);
    assert.match(source, /func allows\(origin: String, method: String, path: String, headers: \[String\]\)/);
    assert.match(source, /if let pathPrefix, !path\.hasPrefix\(pathPrefix\)/);
    assert.match(source, /path: PlatformNetwork\.path\(for: url\)/);
  }
  const macDevControl = read("native/macos/Sources/NativeAIHostMac/DevControlPlane.swift");
  assert.match(macDevControl, /path: PlatformNetwork\.path\(for: url\)/);
});

test("Android native network policy enforces allow pathPrefix", () => {
  const source = read("native/android/app/src/main/java/com/nativeai/platform/PlatformNetwork.kt");

  assert.match(source, /val pathPrefix: String\?/);
  assert.match(source, /pathPrefix = raw\.optString\("pathPrefix", ""\)\.ifBlank \{ null \}/);
  assert.match(source, /fun allows\(origin: String, method: String, path: String, headers: Set<String>\)/);
  assert.match(source, /pathPrefix != null && !path\.startsWith\(pathPrefix\)/);
  assert.match(source, /path\(nextUrl\)/);
});

test("Linux native network policy enforces allow pathPrefix", () => {
  const header = read("native/linux/src/bridge_types.h");
  const types = read("native/linux/src/bridge_types.c");
  const host = read("native/linux/src/webkit_host.c");
  const network = read("native/linux/src/platform_network.c");

  assert.match(header, /gchar \*path_prefix/);
  assert.match(types, /g_clear_pointer\(&rule->path_prefix, g_free\)/);
  assert.match(host, /json_object_get_string_member\(raw, "pathPrefix"\)/);
  assert.match(network, /path_for_uri/);
  assert.match(network, /g_uri_resolve_relative/);
  assert.match(network, /rule->path_prefix != NULL && rule->path_prefix\[0\] != '\\0'/);
  assert.match(network, /!g_str_has_prefix/);
  assert.match(network, /find_rule\(request->context\.network_policy, origin, method, path, headers\)/);
  assert.match(network, /find_rule\(request->context\.network_policy, next_origin, method, next_path, headers\)/);
});

test("Windows native network policy enforces allow pathPrefix", () => {
  const bridgeTypes = read("native/windows/src/BridgeTypes.h");
  const host = read("native/windows/src/WebViewHost.cpp");
  const network = read("native/windows/src/PlatformNetwork.cpp");

  assert.match(bridgeTypes, /std::wstring pathPrefix/);
  assert.match(host, /raw\.GetNamedString\(L"pathPrefix", L""\)/);
  assert.match(network, /std::wstring policyPath/);
  assert.match(network, /ResolveRedirectUrl/);
  assert.match(network, /location\.starts_with\(L"\/\/"\)/);
  assert.match(network, /BaseDirectoryPath\(current\.policyPath\)/);
  assert.match(network, /!rule\.pathPrefix\.empty\(\) && path\.rfind\(rule\.pathPrefix, 0\) != 0/);
  assert.match(network, /FindRule\(request\.context\.networkPolicy, parsed->origin, method, parsed->policyPath, headers\.value\(\)\)/);
  assert.match(network, /FindRule\(request\.context\.networkPolicy, nextParsed->origin, method, nextParsed->policyPath, headers\.value\(\)\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
