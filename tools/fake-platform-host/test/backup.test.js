import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("backup export/import round-trips active app and storage with re-sign", async () => {
  const source = new FakePlatformHost({ keyFile: false });
  const target = new FakePlatformHost({ keyFile: false });
  try {
    const install = source.installPackage(path.join(examplesDir, "notes-lite"));
    await source.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Portable note" }],
    });

    const backup = await source.runControlCommand("db.export_backup", {});
    assert.equal(backup.type, "backup");
    assert.equal(backup.apps.length, 1);
    assert.equal(backup.appVersions[0].install_id, install.installId);
    assert.match(backup.contentHash, /^sha256:[a-f0-9]{64}$/);

    const imported = await target.runControlCommand("db.import_backup", { backup });
    assert.equal(imported.ok, true);
    assert.equal(imported.apps, 1);
    assert.equal(imported.appStorage, 1);

    const restored = await target.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(restored.result.value[0].title, "Portable note");

    const opened = await target.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    assert.equal(opened.appId, "notes-lite");

    const versions = await target.runControlCommand("platform.list_webapp_versions", { appId: "notes-lite" });
    const signature = versions[0].signature;
    assert.notEqual(signature.keyId, source.keypair.keyId);
    assert.equal(signature.keyId, target.keypair.keyId);
  } finally {
    source.close();
    target.close();
  }
});

test("debug bundle export includes runtime diagnostics", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });

    const bundle = await host.runControlCommand("db.export_debug_bundle", {});
    assert.equal(bundle.type, "debug-bundle");
    assert.equal(bundle.debug.runtimeSessions.length >= 1, true);
    assert.equal(bundle.debug.bridgeCalls.length >= 1, true);
  } finally {
    host.close();
  }
});
