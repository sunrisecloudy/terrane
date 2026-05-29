import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { listZipEntries, packageReleaseArtifacts } from "../../../tools/package-release.mjs";

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
    assert.equal(firstManifest.artifacts.some((artifact) => artifact.id === "native-apps"), true);
  } finally {
    fs.rmSync(outDir, { recursive: true, force: true });
  }
});
