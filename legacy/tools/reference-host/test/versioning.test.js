import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { PlatformError } from "../src/errors.js";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("reference host preserves immutable versions and rolls back active install", async () => {
  const host = new ReferenceHost();
  const updatedDir = fs.mkdtempSync(path.join(os.tmpdir(), "notes-lite-update-"));
  fs.cpSync(path.join(examplesDir, "notes-lite"), updatedDir, { recursive: true });
  const manifestPath = path.join(updatedDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.version = "0.2.0";
  manifest.description = "Updated notes package for rollback testing.";
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    const second = host.installPackage(updatedDir);
    assert.notEqual(first.installId, second.installId);

    const before = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    assert.equal(before.length, 2);
    assert.equal(before[0].installId, second.installId);
    assert.equal(before[0].status, "enabled");
    assert.equal(before[1].installId, first.installId);
    assert.equal(before[1].status, "installed");

    const rollback = await host.runControlCommand("platform.rollback_webapp", { appId: "notes-lite" });
    assert.equal(rollback.activeInstallId, first.installId);
    assert.equal(rollback.rolledBackInstallId, second.installId);

    const after = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    const firstAfter = after.find((version) => version.installId === first.installId);
    const secondAfter = after.find((version) => version.installId === second.installId);
    assert.equal(firstAfter.status, "enabled");
    assert.equal(secondAfter.status, "rolled-back");

    const report = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: first.installId });
    assert.equal(report.installId, first.installId);
    assert.equal(report.status, "accepted");
  } finally {
    host.close();
  }
});

test("quarantined active app refuses verified open", async () => {
  const host = new ReferenceHost();
  try {
    const install = host.installPackage(path.join(examplesDir, "notes-lite"));
    const quarantine = await host.runControlCommand("platform.quarantine_webapp", {
      appId: "notes-lite",
      installId: install.installId,
      reason: "test quarantine",
    });
    assert.equal(quarantine.status, "quarantined");

    await assert.rejects(
      () => host.runControlCommand("platform.open_webapp", { appId: "notes-lite" }),
      (error) => error instanceof PlatformError && error.code === "package_quarantined",
    );
  } finally {
    host.close();
  }
});
