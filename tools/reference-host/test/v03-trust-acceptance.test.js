import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { PlatformError } from "../src/errors.js";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("v0.3 install trust flow signs, reports, preserves versions, and rejects tampering", async () => {
  const host = new ReferenceHost();
  const updatedDir = fs.mkdtempSync(path.join(os.tmpdir(), "notes-lite-v03-trust-"));
  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    const firstReport = await host.runControlCommand("platform.install_report", {
      appId: "notes-lite",
      installId: first.installId,
    });
    assert.equal(firstReport.reportId, first.reportId);
    assert.equal(firstReport.status, "accepted");
    assert.equal(firstReport.security.ok, true);
    assert.equal(firstReport.security.signature.algorithm, "ed25519");
    assert.equal(firstReport.security.signature.appId, "notes-lite");
    assert.equal(firstReport.security.signature.contentHash, first.contentHash);
    assert.equal(firstReport.contentHash, first.contentHash);

    fs.cpSync(path.join(examplesDir, "notes-lite"), updatedDir, { recursive: true });
    const manifestPath = path.join(updatedDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.version = "0.2.0";
    manifest.description = "Updated notes package for v0.3 trust acceptance.";
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const second = host.installPackage(updatedDir);
    assert.notEqual(first.installId, second.installId);

    const versions = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    const firstVersion = versions.find((version) => version.installId === first.installId);
    const secondVersion = versions.find((version) => version.installId === second.installId);
    assert.equal(firstVersion.appVersion, "0.1.0");
    assert.equal(firstVersion.status, "installed");
    assert.equal(firstVersion.contentHash, first.contentHash);
    assert.equal(firstVersion.signature.algorithm, "ed25519");
    assert.equal(secondVersion.appVersion, "0.2.0");
    assert.equal(secondVersion.status, "enabled");
    assert.equal(secondVersion.contentHash, second.contentHash);

    const appRow = host.database.snapshot().apps.find((app) => app.id === "notes-lite");
    assert.equal(appRow.active_install_id, second.installId);
    assert.equal(appRow.active_version, "0.2.0");

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");

    host.database.run(
      "UPDATE app_files SET content_text = content_text || ? WHERE install_id = ? AND path = 'app.js'",
      "\n// tampered after v0.3 trust acceptance",
      second.installId,
    );

    await assert.rejects(
      () => host.runControlCommand("platform.open_webapp", { appId: "notes-lite" }),
      (error) => error instanceof PlatformError && error.code === "content_tampered",
    );
  } finally {
    host.close();
  }
});
