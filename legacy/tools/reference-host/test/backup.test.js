import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";
import { readPackage } from "../src/package-validator.js";
import { signPackage } from "../src/signing.js";

test("backup export/import round-trips active app and storage with re-sign", async () => {
  const source = new ReferenceHost({ keyFile: false });
  const target = new ReferenceHost({ keyFile: false });
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

test("backup export/import round-trips notebook CRDT state", async () => {
  const source = new ReferenceHost({ keyFile: false });
  const target = new ReferenceHost({ keyFile: false });
  const notebookId = "backup_notebook";
  try {
    installNotebookPackage(source);
    await source.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "notebook.open",
      params: { notebookId, title: "Portable notebook" },
    });
    await source.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "notebook.apply_local",
      params: {
        notebookId,
        operation: {
          opId: "op_backup_cell",
          type: "cell.insert",
          cellId: "cell_backup",
          cellType: "markdown",
          source: "CRDT backup survives",
        },
      },
    });

    const backup = await source.runControlCommand("db.export_backup", {});
    assert.equal(backup.crdtNotebooks.length, 1);
    assert.equal(backup.crdtDocuments.length, 1);
    assert.equal(backup.crdtUpdates.length, 1);
    assert.equal(backup.crdtHeads[0].version, 1);
    assert.equal(backup.crdtPermissions.some((permission) => permission.permission === "notebook.write"), true);

    const imported = await target.runControlCommand("db.import_backup", { backup });
    assert.equal(imported.ok, true);
    assert.equal(imported.crdtUpdates, 1);

    const restored = await target.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "notebook.snapshot",
      params: { notebookId },
    });
    assert.equal(restored.ok, true);
    assert.equal(restored.result.notebook.cells[0].source, "CRDT backup survives");
    assert.equal(target.database.snapshot().crdt_updates.length, 1);
  } finally {
    source.close();
    target.close();
  }
});

test("debug bundle export includes runtime diagnostics", async () => {
  const host = new ReferenceHost();
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

function installNotebookPackage(host) {
  const pkg = readPackage(path.join(examplesDir, "notes-lite"));
  const manifest = {
    ...pkg.manifest,
    permissions: [
      ...pkg.manifest.permissions,
      "notebook.read",
      "notebook.write",
      "notebook.propose",
      "notebook.approve",
      "notebook.sync",
    ],
    capabilities: {
      required: [...pkg.manifest.capabilities.required, "notebook.read"],
      optional: [
        ...pkg.manifest.capabilities.optional,
        "notebook.write",
        "notebook.propose",
        "notebook.approve",
        "notebook.sync",
      ],
    },
  };
  const signed = signPackage({ manifest, files: pkg.files, keypair: host.keypair });
  return host.database.insertInstalledPackage({
    manifest,
    files: pkg.files,
    hashes: signed.hashes,
    validation: pkg.validation,
    signature: signed.signature,
    contentHashesDocument: signed.contentHashesDocument,
  });
}
