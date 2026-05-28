import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("migration dry-run creates snapshot without changing storage, apply updates data", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "task-workbench"));
    await host.runControlCommand("runtime.storage_set", {
      appId: "task-workbench",
      key: "task-workbench:tasks",
      value: [{ id: "t1", title: "Write tests" }],
    });

    const migration = {
      appId: "task-workbench",
      fromDataVersion: 1,
      toDataVersion: 2,
      steps: [{ op: "setDefault", key: "task-workbench:tasks", to: "archived", value: false }],
    };

    const dryRun = await host.runControlCommand("platform.migration_dry_run", { migration });
    assert.equal(dryRun.status, "passed");
    assert.deepEqual(dryRun.changedKeys, ["task-workbench:tasks"]);
    assert.match(dryRun.snapshotId, /^snapshot_/);

    const beforeApply = await host.runControlCommand("runtime.storage_get", {
      appId: "task-workbench",
      key: "task-workbench:tasks",
      defaultValue: [],
    });
    assert.equal(beforeApply.result.value[0].archived, undefined);

    const applied = await host.runControlCommand("platform.migration_apply", { migration });
    assert.equal(applied.status, "passed");

    const afterApply = await host.runControlCommand("runtime.storage_get", {
      appId: "task-workbench",
      key: "task-workbench:tasks",
      defaultValue: [],
    });
    assert.equal(afterApply.result.value[0].archived, false);

    const snapshot = await host.runControlCommand("db.snapshot", {});
    assert.equal(snapshot.migration_runs.length, 2);
    assert.equal(snapshot.runtime_snapshots.length, 2);
  } finally {
    host.close();
  }
});

test("restore snapshot reverts app storage", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Before" }],
    });
    const snapshot = await host.runControlCommand("platform.create_snapshot", { appId: "notes-lite" });
    await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "After" }],
    });

    await host.runControlCommand("platform.restore_snapshot", { snapshotId: snapshot.snapshotId });
    const restored = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(restored.result.value[0].title, "Before");
  } finally {
    host.close();
  }
});
