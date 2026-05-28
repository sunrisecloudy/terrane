import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("permission-changing update waits for approval before activation", async () => {
  const host = new FakePlatformHost();
  const updatePackage = fs.mkdtempSync(path.join(os.tmpdir(), "approval-update-package-"));
  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    fs.cpSync(path.join(examplesDir, "notes-lite"), updatePackage, { recursive: true });
    const manifestPath = path.join(updatePackage, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.permissions = [...manifest.permissions, "network.request"];
    manifest.capabilities.optional = [...manifest.capabilities.optional, "network.request"];
    manifest.networkPolicy = {
      allow: [{ origin: "https://api.example.test", methods: ["GET"], pathPrefix: "/status" }],
    };
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const pending = host.installPackage(updatePackage);
    assert.equal(pending.status, "requires-approval");
    assert.equal(pending.approval.requiresUserApproval, true);
    assert.deepEqual(pending.approval.reasons.sort(), ["capabilities", "networkPolicy", "permissions"]);
    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);

    const report = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: pending.installId });
    assert.equal(report.status, "requires-approval");
    assert.equal(report.requiresUserApproval, true);
    assert.deepEqual(report.permissions.approvalReasons.sort(), ["capabilities", "networkPolicy", "permissions"]);
    assert.deepEqual(report.permissions.approved, []);

    const versionsBeforeApproval = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    assert.equal(versionsBeforeApproval.find((version) => version.installId === pending.installId).status, "installed");

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");
    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);

    const approved = await host.runControlCommand("platform.approve_webapp_update", {
      appId: "notes-lite",
      installId: pending.installId,
    });
    assert.equal(approved.status, "enabled");
    assert.equal(approved.previousInstallId, first.installId);
    assert.equal(host.database.activeInstallId("notes-lite"), pending.installId);

    const approvedReport = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: pending.installId });
    assert.equal(approvedReport.status, "accepted");
    assert.equal(approvedReport.requiresUserApproval, true);
    assert.equal(approvedReport.permissions.approvalGranted, true);
    assert.equal(approvedReport.permissions.approved.includes("network.request"), true);
  } finally {
    host.close();
  }
});
