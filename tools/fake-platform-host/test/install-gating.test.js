import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("install runs bundled smoke tests before activation", async () => {
  const host = new FakePlatformHost();
  try {
    const install = host.installPackage(path.join(examplesDir, "notes-lite"));
    assert.equal(install.status, "enabled");
    assert.equal(install.smokeTest.status, "passed");

    const report = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: install.installId });
    assert.equal(report.status, "accepted");
    assert.equal(report.smokeTest.status, "passed");

    const runs = await host.runControlCommand("db.query_test_runs", { appId: "notes-lite" });
    assert.equal(runs.length, 1);
    assert.equal(runs[0].micro_test_id, "smoke:notes-lite");
    assert.equal(runs[0].status, "passed");
  } finally {
    host.close();
  }
});

test("failing install smoke test quarantines new version and preserves active version", async () => {
  const host = new FakePlatformHost();
  const badPackage = fs.mkdtempSync(path.join(os.tmpdir(), "bad-smoke-package-"));
  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    fs.cpSync(path.join(examplesDir, "notes-lite"), badPackage, { recursive: true });
    fs.writeFileSync(
      path.join(badPackage, "smoke-tests.json"),
      JSON.stringify(
        [
          {
            name: "broken selector",
            steps: [{ type: "click", selector: "#missing-control" }],
            expected: { textIncludes: "No notes yet" },
          },
        ],
        null,
        2,
      ),
    );

    const failed = host.installPackage(badPackage);
    assert.equal(failed.status, "quarantined");
    assert.equal(failed.smokeTest.status, "failed");
    assert.equal(failed.smokeTest.failures[0].code, "selector.not_found");

    const versions = await host.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    const quarantined = versions.find((version) => version.installId === failed.installId);
    assert.equal(quarantined.status, "quarantined");

    const opened = await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");
    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);

    const failedReport = await host.runControlCommand("platform.install_report", { appId: "notes-lite", installId: failed.installId });
    assert.equal(failedReport.status, "failed");
    assert.equal(failedReport.smokeTest.status, "failed");
  } finally {
    host.close();
  }
});
