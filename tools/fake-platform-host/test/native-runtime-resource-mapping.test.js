import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function readRepoFile(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function assertContains(source, snippet, label) {
  assert.equal(source.includes(snippet), true, `${label} must contain ${JSON.stringify(snippet)}`);
}

test("Apple native hosts route runtime and generated-app resources through app-runtime scheme", () => {
  for (const [label, relativePath] of [
    ["macOS", "native/macos/Sources/NativeAIHostMac/WebHostView.swift"],
    ["iOS", "native/ios/Sources/NativeAIHostIOS/WebHostView.swift"],
  ]) {
    const source = readRepoFile(relativePath);

    assertContains(source, "WKURLSchemeHandler", label);
    assertContains(source, "configuration.setURLSchemeHandler", label);
    assertContains(source, 'static let scheme = "app-runtime"', label);
    assertContains(source, 'URL(string: "\\(scheme)://runtime/index.html")!', label);
    assertContains(source, 'appendingPathComponent("runtime-web")', label);
    assertContains(source, 'logicalPath.hasPrefix("runtime/")', label);
    assertContains(source, 'logicalPath.hasPrefix("webapps/examples/")', label);
    assertContains(source, "!path.contains(\"..\")", label);
    assertContains(source, "mimeType(for:", label);
    assert.equal(source.includes("loadFileURL("), false, `${label} must not load runtime as file:// with absolute /runtime paths`);
  }
});

test("Linux native host maps app-runtime /runtime paths to runtime-web files", () => {
  const source = readRepoFile("native/linux/src/webkit_host.c");

  assertContains(source, 'static const gchar *k_runtime_scheme = "app-runtime"', "Linux");
  assertContains(source, "logical_path_for_runtime_uri", "Linux");
  assertContains(source, 'g_strcmp0(host, "runtime") == 0', "Linux");
  assertContains(source, '"runtime/index.html"', "Linux");
  assertContains(source, '"runtime-web"', "Linux");
  assertContains(source, 'g_str_has_prefix(logical_path, "runtime/")', "Linux");
  assertContains(source, 'g_str_has_prefix(path, "webapps/examples/")', "Linux");
  assertContains(source, 'strstr(path, "..") == NULL', "Linux");
  assertContains(source, "content_type_for_path", "Linux");
  assertContains(source, 'webkit_web_view_load_uri(host->web_view, "app-runtime://runtime/index.html")', "Linux");
  assertContains(source, "is_trusted_runtime_uri", "Linux");
  assertContains(source, 'g_str_has_prefix(uri, "app-runtime://runtime/")', "Linux");
  assertContains(source, "if (!is_trusted_runtime_uri(uri))", "Linux");
  assert.equal(source.includes('!g_str_has_prefix(uri, "app-runtime://runtime-web/")'), false, "Linux bridge must trust the loaded runtime URI");
});

test("Android native host packages runtime-web under the /runtime asset path", () => {
  const gradle = readRepoFile("native/android/app/build.gradle.kts");
  const activity = readRepoFile("native/android/app/src/main/java/com/nativeai/platform/MainActivity.kt");

  assertContains(gradle, 'from(repoRoot.resolve("runtime-web"))', "Android Gradle");
  assertContains(gradle, 'into("runtime")', "Android Gradle");
  assertContains(gradle, 'from(repoRoot.resolve("webapps"))', "Android Gradle");
  assertContains(activity, 'webView.loadUrl("https://appassets.androidplatform.net/runtime/index.html")', "Android Activity");
  assertContains(activity, 'path.startsWith("runtime/")', "Android Activity");
  assertContains(activity, 'path.startsWith("webapps/examples/")', "Android Activity");
});

test("Windows native host stages runtime-web under the /runtime WebView2 path", () => {
  const cmake = readRepoFile("native/windows/CMakeLists.txt");
  const host = readRepoFile("native/windows/src/WebViewHost.cpp");

  assertContains(cmake, 'copy_directory "${NATIVE_AI_REPO_ROOT}/runtime-web"', "Windows CMake");
  assertContains(cmake, "resources/runtime", "Windows CMake");
  assertContains(cmake, 'copy_directory "${NATIVE_AI_REPO_ROOT}/webapps/examples"', "Windows CMake");
  assertContains(cmake, "resources/webapps/examples", "Windows CMake");
  assertContains(host, 'webview_->Navigate(L"https://runtime.local.platform/runtime/index.html")', "Windows host");
  assertContains(host, 'resourceRoot / L"runtime" / L"index.html"', "Windows host");
  assertContains(host, 'resourceRoot / L"webapps" / L"examples"', "Windows host");
});
