import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { listZipEntries, packageReleaseArtifacts, windowsWebView2SdkStatus } from "../../../tools/package-release.mjs";

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasSwift() {
  try {
    execFileSync("swift", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasCmake() {
  try {
    execFileSync("cmake", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasMeson() {
  try {
    execFileSync("meson", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasNinja() {
  try {
    execFileSync("ninja", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function hasLinuxNativeDependencies() {
  try {
    execFileSync("pkg-config", ["--exists", "gtk4", "webkitgtk-6.0", "json-glib-1.0", "sqlite3", "libsoup-3.0"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function linuxReleaseSkipReason() {
  if (process.platform !== "linux") return "Linux native release artifact only builds on Linux hosts";
  if (process.arch !== "x64") return "Linux native release artifact currently requires an x64 Linux host";
  if (!hasMeson()) return "meson is not available";
  if (!hasNinja()) return "ninja is not available";
  if (!hasZig()) return "zig is not available";
  if (!hasLinuxNativeDependencies()) return "GTK/WebKitGTK development dependencies are not available";
  return false;
}

function windowsReleaseSkipReason() {
  if (process.platform !== "win32") return "Windows native release artifact only builds on Windows hosts";
  if (process.arch !== "x64") return "Windows native release artifact currently requires an x64 Windows host";
  if (!hasCmake()) return "cmake is not available";
  if (!hasZig()) return "zig is not available";

  const sdkStatus = windowsWebView2SdkStatus();
  return sdkStatus.ok ? false : sdkStatus.message;
}

test("release packaging creates deterministic static artifact archives and manifest", () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-artifacts-"));
  try {
    const first = packageReleaseArtifacts({ outDir });
    const firstManifest = JSON.parse(fs.readFileSync(first.manifestPath, "utf8"));
    const firstRuntimeHash = firstManifest.artifacts.find((artifact) => artifact.id === "runtime-web").sha256;
    const firstExamplesHash = firstManifest.artifacts.find((artifact) => artifact.id === "example-webapps").sha256;
    const firstPublicContractHash = firstManifest.artifacts.find((artifact) => artifact.id === "public-contract").sha256;

    const second = packageReleaseArtifacts({ outDir });
    const secondManifest = JSON.parse(fs.readFileSync(second.manifestPath, "utf8"));
    assert.equal(secondManifest.artifacts.find((artifact) => artifact.id === "runtime-web").sha256, firstRuntimeHash);
    assert.equal(secondManifest.artifacts.find((artifact) => artifact.id === "example-webapps").sha256, firstExamplesHash);
    assert.equal(secondManifest.artifacts.find((artifact) => artifact.id === "public-contract").sha256, firstPublicContractHash);

    const runtimeEntries = listZipEntries(path.join(outDir, "runtime-web.zip"));
    assert.deepEqual(runtimeEntries, [...runtimeEntries].sort());
    assert.ok(runtimeEntries.includes("runtime-web/index.html"));
    assert.ok(runtimeEntries.includes("runtime-web/runtime.js"));

    const exampleEntries = listZipEntries(path.join(outDir, "example-webapps.zip"));
    assert.deepEqual(exampleEntries, [...exampleEntries].sort());
    assert.ok(exampleEntries.includes("webapps/examples/notes-lite/manifest.json"));
    assert.ok(exampleEntries.includes("webapps/examples/task-workbench/app.js"));

    for (const target of ["ios", "macos", "android", "windows", "linux"]) {
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", target, "README.txt")), true);
    }
    assert.equal(fs.existsSync(path.join(outDir, "server", "README.txt")), true);
    assert.equal(fs.existsSync(path.join(outDir, "native-apps", "README.txt")), true);

    assert.equal(firstManifest.platformVersion, "0.1.0");
    const publicContractArtifact = firstManifest.artifacts.find((artifact) => artifact.id === "public-contract");
    assert.equal(publicContractArtifact.kind, "json");
    assert.equal(publicContractArtifact.path, "public-contract.json");
    assert.match(publicContractArtifact.sha256, /^[a-f0-9]{64}$/);
    const publicContract = JSON.parse(fs.readFileSync(path.join(outDir, "public-contract.json"), "utf8"));
    assert.equal(publicContract.contractId, "terrane-public-contract");
    assert.equal(publicContract.files.contracts.some((file) => file.path === "forge/contracts/public-contract.schema.json"), true);
    assert.equal(publicContract.files.docs.some((file) => file.path === "docs/35_PUBLIC_CONTRACT_EXPORT.md"), true);
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "zig-core-windows"), true);
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "server" && artifact.kind === "directory"), true);
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "native-apps"), true);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});

test(
  "release packaging can build Zig core libraries for platform artifact targets",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 60_000,
  },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-zig-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildZigCore: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const coreArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "zig-core-library");
      assert.equal(coreArtifacts.length, 8);

      for (const artifact of coreArtifacts) {
        assert.equal(fs.existsSync(path.join(outDir, artifact.path, "zig_core.h")), true);
        assert.equal(artifact.files.some((file) => file.path.endsWith("zig_core.h") && file.sha256.length === 64), true);
      }

      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "ios", "ios-arm64-device", "libzig_core.a")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "ios", "ios-arm64-simulator", "libzig_core.a")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "macos", "macos-arm64", "libzig_core.a")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "macos", "macos-x86_64", "libzig_core.a")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "android", "android-arm64-v8a", "libzig_core.so")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "android", "android-x86_64", "libzig_core.so")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "linux", "linux-x86_64", "libzig_core.so")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "windows", "windows-x86_64", "zig_core.dll")), true);
      assert.equal(fs.existsSync(path.join(outDir, "zig-core", "windows", "windows-x86_64", "zig_core.lib")), true);
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);

test(
  "release packaging can build the macOS native host app artifact",
  {
    skip: process.platform !== "darwin"
      ? "macOS native release artifact only builds on Darwin hosts"
      : !hasSwift()
        ? "swift is not available"
        : !hasZig()
          ? "zig is not available"
          : false,
    timeout: 120_000,
  },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-macos-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildNativeMacOS: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const nativeArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "native-host-app");
      assert.equal(nativeArtifacts.length, 1);
      const dmgArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "dmg");
      assert.equal(dmgArtifacts.length, 1);

      const [nativeArtifact] = nativeArtifacts;
      assert.match(nativeArtifact.target, /^macos-(arm64|x86_64)$/);
      for (const relativePath of [
        "Contents/MacOS/TerraneHostMac",
        "Contents/Resources/runtime/index.html",
        "Contents/Resources/webapps/examples/notes-lite/manifest.json",
        "Contents/Resources/db/sqlite/001_initial.sql",
        "Contents/Frameworks/libzig_core.dylib",
      ]) {
        const manifestPath = path.join(nativeArtifact.path, relativePath).split(path.sep).join("/");
        assert.equal(nativeArtifact.files.some((file) => file.path === manifestPath && file.sha256.length === 64), true);
        assert.equal(fs.existsSync(path.join(outDir, manifestPath)), true);
      }

      const [dmgArtifact] = dmgArtifacts;
      assert.equal(dmgArtifact.id, `native-macos-${nativeArtifact.target}-dmg`);
      assert.equal(dmgArtifact.target, nativeArtifact.target);
      assert.equal(dmgArtifact.path, `native-apps/macos/${nativeArtifact.target}/Terrane-${nativeArtifact.target}.dmg`);
      assert.equal(dmgArtifact.appBundle, nativeArtifact.path);
      assert.match(dmgArtifact.sha256, /^[a-f0-9]{64}$/);
      assert.equal(dmgArtifact.bytes > 0, true);
      assert.equal(fs.existsSync(path.join(outDir, dmgArtifact.path)), true);
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);

test(
  "release packaging can build the Linux native host app artifact",
  {
    skip: linuxReleaseSkipReason(),
    timeout: 180_000,
  },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-linux-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildNativeLinux: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const nativeArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "native-host-app");
      assert.equal(nativeArtifacts.length, 1);

      const [nativeArtifact] = nativeArtifacts;
      assert.equal(nativeArtifact.id, "native-linux-linux-x86_64");
      assert.equal(nativeArtifact.target, "linux-x86_64");
      assert.equal(nativeArtifact.path, "native-apps/linux/linux-x86_64/TerraneHost");
      for (const relativePath of [
        "terrane-host",
        "libzig_core.so",
        "resources/runtime/index.html",
        "resources/runtime/runtime.js",
        "resources/webapps/examples/notes-lite/manifest.json",
        "resources/webapps/examples/task-workbench/app.js",
        "resources/db/sqlite/001_initial.sql",
      ]) {
        const manifestPath = path.join(nativeArtifact.path, relativePath).split(path.sep).join("/");
        const file = nativeArtifact.files.find((entry) => entry.path === manifestPath);
        assert.notEqual(file, undefined);
        assert.match(file.sha256, /^[a-f0-9]{64}$/);
        assert.equal(file.bytes > 0, true);
        assert.equal(fs.existsSync(path.join(outDir, manifestPath)), true);
      }

      for (const relativePath of ["terrane-host", "libzig_core.so"]) {
        assert.notEqual(fs.statSync(path.join(outDir, nativeArtifact.path, relativePath)).mode & 0o111, 0);
      }
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);

test(
  "release packaging can build the Windows native host app artifact",
  {
    skip: windowsReleaseSkipReason(),
    timeout: 180_000,
  },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-windows-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildNativeWindows: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const nativeArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "native-host-app");
      assert.equal(nativeArtifacts.length, 1);

      const [nativeArtifact] = nativeArtifacts;
      assert.equal(nativeArtifact.target, "windows-x86_64");
      assert.equal(nativeArtifact.path, "native-apps/windows/windows-x86_64/TerraneHost");
      for (const relativePath of [
        "TerraneHost.exe",
        "zig_core.dll",
        "resources/runtime/index.html",
        "resources/webapps/examples/notes-lite/manifest.json",
        "resources/db/sqlite/001_initial.sql",
      ]) {
        const manifestPath = path.join(nativeArtifact.path, relativePath).split(path.sep).join("/");
        const file = nativeArtifact.files.find((entry) => entry.path === manifestPath);
        assert.notEqual(file, undefined);
        assert.match(file.sha256, /^[a-f0-9]{64}$/);
        assert.equal(fs.existsSync(path.join(outDir, manifestPath)), true);
      }
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);

test(
  "release packaging can build the Forge server executable artifact",
  { timeout: 60_000 },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-server-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildServer: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const serverArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "forge-server-executable");
      assert.equal(serverArtifacts.length, 1);
      assert.equal(manifest.artifacts.some((artifact) => artifact.id === "server" && artifact.kind === "directory"), false);

      const [serverArtifact] = serverArtifacts;
      assert.match(serverArtifact.target, /^(linux|macos|windows)-(arm64|x86_64)$/);
      assert.equal(serverArtifact.files.length, 1);
      assert.equal(serverArtifact.files[0].path.endsWith(process.platform === "win32" ? "terrane-server.exe" : "terrane-server"), true);
      assert.equal(serverArtifact.files[0].sha256.length, 64);
      assert.equal(fs.existsSync(path.join(outDir, serverArtifact.files[0].path)), true);
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);
