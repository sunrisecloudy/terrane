import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("v0.4 database scopes app storage and versions permissions per install id", async () => {
  const host = new ReferenceHost();
  const updatePackage = fs.mkdtempSync(path.join(os.tmpdir(), "notes-lite-v04-permissions-"));
  try {
    const notesInstall = host.installPackage(path.join(examplesDir, "notes-lite"));
    host.installPackage(path.join(examplesDir, "task-workbench"));

    const notesWrite = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Scoped note" }],
    });
    assert.equal(notesWrite.ok, true);

    const tasksWrite = await host.runControlCommand("runtime.storage_set", {
      appId: "task-workbench",
      key: "task-workbench:tasks",
      value: [{ id: "t1", title: "Scoped task" }],
    });
    assert.equal(tasksWrite.ok, true);

    const crossAppRead = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "task-workbench:tasks",
      defaultValue: [],
    });
    assert.equal(crossAppRead.ok, false);
    assert.equal(crossAppRead.error.code, "permission_denied");

    const storageRows = host.database.all("SELECT app_id, key, value_json FROM app_storage ORDER BY app_id, key");
    assert.deepEqual(
      storageRows.map((row) => `${row.app_id}:${row.key}`),
      [
        "notes-lite:notes-lite:notes",
        "task-workbench:task-workbench:tasks",
      ],
    );
    assert.equal(storageRows.every((row) => row.key.startsWith(`${row.app_id}:`)), true);

    const notesRows = await host.runControlCommand("db.query_app_storage", { appId: "notes-lite" });
    assert.deepEqual(notesRows.map((row) => row.key), ["notes-lite:notes"]);

    fs.cpSync(path.join(examplesDir, "notes-lite"), updatePackage, { recursive: true });
    const manifestPath = path.join(updatePackage, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.version = "0.2.0";
    manifest.description = "Permission versioning acceptance update.";
    manifest.permissions = [...manifest.permissions, "network.request"];
    manifest.capabilities.optional = [...manifest.capabilities.optional, "network.request"];
    manifest.networkPolicy = {
      allow: [{ origin: "https://api.example.test", methods: ["GET"], pathPrefix: "/status" }],
    };
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

    const pending = host.installPackage(updatePackage);
    assert.equal(pending.status, "requires-approval");

    const permissionRowsBefore = host.database.all(
      "SELECT install_id, permission, approved FROM app_permissions WHERE app_id = ? ORDER BY install_id, permission",
      "notes-lite",
    );
    const firstPermissions = permissionRowsBefore.filter((row) => row.install_id === notesInstall.installId);
    const pendingPermissions = permissionRowsBefore.filter((row) => row.install_id === pending.installId);
    assert.equal(firstPermissions.every((row) => row.approved === 1), true);
    assert.equal(firstPermissions.some((row) => row.permission === "network.request"), false);
    assert.equal(pendingPermissions.some((row) => row.permission === "network.request" && row.approved === 0), true);
    assert.equal(pendingPermissions.every((row) => row.approved === 0), true);

    const approved = await host.runControlCommand("platform.approve_webapp_update", {
      appId: "notes-lite",
      installId: pending.installId,
    });
    assert.equal(approved.status, "enabled");

    const permissionRowsAfter = host.database.all(
      "SELECT install_id, permission, approved FROM app_permissions WHERE app_id = ? ORDER BY install_id, permission",
      "notes-lite",
    );
    const firstAfter = permissionRowsAfter.filter((row) => row.install_id === notesInstall.installId);
    const approvedAfter = permissionRowsAfter.filter((row) => row.install_id === pending.installId);
    assert.equal(firstAfter.every((row) => row.approved === 1), true);
    assert.equal(firstAfter.some((row) => row.permission === "network.request"), false);
    assert.equal(approvedAfter.some((row) => row.permission === "network.request" && row.approved === 1), true);
    assert.equal(approvedAfter.every((row) => row.approved === 1), true);
  } finally {
    host.close();
  }
});
