import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { listZipEntries, packageReleaseArtifacts, windowsWebView2SdkStatus } from "../../../tools/package-release.mjs";

function hasCargo() {
  try {
    execFileSync("cargo", ["--version"], { stdio: "ignore" });
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

function hasHdiutilCreate() {
  if (process.platform !== "darwin") return false;
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-hdiutil-check-"));
  try {
    const root = path.join(scratch, "root");
    fs.mkdirSync(root);
    fs.writeFileSync(path.join(root, "README.txt"), "hdiutil smoke\n");
    execFileSync("hdiutil", ["create", "-volname", "TerraneCheck", "-srcfolder", root, "-ov", "-format", "UDZO", path.join(scratch, "check.dmg")], {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
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
  if (!hasCargo()) return "cargo is not available";
  if (!hasLinuxNativeDependencies()) return "GTK/WebKitGTK development dependencies are not available";
  return false;
}

function windowsReleaseSkipReason() {
  if (process.platform !== "win32") return "Windows native release artifact only builds on Windows hosts";
  if (process.arch !== "x64") return "Windows native release artifact currently requires an x64 Windows host";
  if (!hasCmake()) return "cmake is not available";
  if (!hasCargo()) return "cargo is not available";

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
      assert.equal(fs.existsSync(path.join(outDir, "forge-ffi", target, "README.txt")), true);
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
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "forge-ffi-windows"), true);
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "server" && artifact.kind === "directory"), true);
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "native-apps"), true);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});

test(
  "release packaging can build the Forge FFI library artifact",
  { timeout: 60_000 },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "terrane-release-forge-ffi-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildForgeFfi: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const ffiArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "forge-ffi-library");
      assert.equal(ffiArtifacts.length, 1);

      const [ffiArtifact] = ffiArtifacts;
      assert.match(ffiArtifact.target, /^(aarch64|x86_64)-(apple-darwin|unknown-linux-gnu|pc-windows-msvc)$/);
      assert.equal(fs.existsSync(path.join(outDir, ffiArtifact.path, "forge_ffi.h")), true);
      assert.equal(ffiArtifact.files.some((file) => file.path.endsWith("forge_ffi.h") && file.sha256.length === 64), true);

      const libraryFiles = ffiArtifact.files.filter((file) => !file.path.endsWith("forge_ffi.h"));
      assert.equal(libraryFiles.length > 0, true);
      for (const file of libraryFiles) {
        assert.match(file.sha256, /^[a-f0-9]{64}$/);
        assert.equal(fs.existsSync(path.join(outDir, file.path)), true);
      }
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
        : !hasCargo()
          ? "cargo is not available"
          : !hasHdiutilCreate()
            ? "hdiutil create is not available"
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
        "Contents/Info.plist",
        "Contents/Resources/runtime/index.html",
        "Contents/Resources/webapps/examples/notes-lite/manifest.json",
        "Contents/Resources/db/sqlite/001_initial.sql",
        "Contents/Frameworks/libforge_ffi.dylib",
      ]) {
        const manifestPath = path.join(nativeArtifact.path, relativePath).split(path.sep).join("/");
        assert.equal(nativeArtifact.files.some((file) => file.path === manifestPath && file.sha256.length === 64), true);
        assert.equal(fs.existsSync(path.join(outDir, manifestPath)), true);
      }
      assert.equal(nativeArtifact.path, `native-apps/macos/${nativeArtifact.target}/terrane.app`);
      const infoPlist = fs.readFileSync(path.join(outDir, nativeArtifact.path, "Contents", "Info.plist"), "utf8");
      assert.match(infoPlist, /<key>CFBundleName<\/key><string>terrane<\/string>/);
      assert.match(infoPlist, /<key>CFBundleDisplayName<\/key><string>terrane<\/string>/);

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
        "libforge_ffi.so",
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

      for (const relativePath of ["terrane-host", "libforge_ffi.so"]) {
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
        "forge_ffi.dll",
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
