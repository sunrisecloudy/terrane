import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { listZipEntries, packageReleaseArtifacts } from "../../../tools/package-release.mjs";

function hasZig() {
  try {
    execFileSync("zig", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test("release packaging creates deterministic static artifact archives and manifest", () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-release-artifacts-"));
  try {
    const first = packageReleaseArtifacts({ outDir });
    const firstManifest = JSON.parse(fs.readFileSync(first.manifestPath, "utf8"));
    const firstRuntimeHash = firstManifest.artifacts.find((artifact) => artifact.id === "runtime-web").sha256;
    const firstExamplesHash = firstManifest.artifacts.find((artifact) => artifact.id === "example-webapps").sha256;

    const second = packageReleaseArtifacts({ outDir });
    const secondManifest = JSON.parse(fs.readFileSync(second.manifestPath, "utf8"));
    assert.equal(secondManifest.artifacts.find((artifact) => artifact.id === "runtime-web").sha256, firstRuntimeHash);
    assert.equal(secondManifest.artifacts.find((artifact) => artifact.id === "example-webapps").sha256, firstExamplesHash);

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
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-release-zig-artifacts-"));
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
  "release packaging can build the host server executable artifact",
  {
    skip: !hasZig() ? "zig is not available" : false,
    timeout: 60_000,
  },
  () => {
    const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-release-server-artifacts-"));
    try {
      const result = packageReleaseArtifacts({ outDir, buildServer: true });
      const manifest = JSON.parse(fs.readFileSync(result.manifestPath, "utf8"));
      const serverArtifacts = manifest.artifacts.filter((artifact) => artifact.kind === "server-executable");
      assert.equal(serverArtifacts.length, 1);
      assert.equal(manifest.artifacts.some((artifact) => artifact.id === "server" && artifact.kind === "directory"), false);

      const [serverArtifact] = serverArtifacts;
      assert.match(serverArtifact.target, /^(linux|macos|windows)-(arm64|x86_64)$/);
      assert.equal(serverArtifact.files.length, 1);
      assert.equal(serverArtifact.files[0].sha256.length, 64);
      assert.equal(fs.existsSync(path.join(outDir, serverArtifact.files[0].path)), true);
    } finally {
      fs.rmSync(outDir, { recursive: true, force: true });
    }
  },
);
