import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("v0.3 fake-host runtime acceptance covers snapshot replay, budgets, and network policy", async () => {
  const host = new FakePlatformHost();
  const budgetDir = fs.mkdtempSync(path.join(os.tmpdir(), "notes-lite-v03-budget-"));
  try {
    host.installPackage(path.join(examplesDir, "task-workbench"));
    await host.runControlCommand("runtime.storage_set", {
      appId: "task-workbench",
      key: "task-workbench:tasks",
      value: [{ id: "t1", title: "Before snapshot" }],
    });

    const snapshot = await host.runControlCommand("platform.create_snapshot", {
      appId: "task-workbench",
      type: "manual",
    });
    assert.match(snapshot.snapshotId, /^snapshot_/);
    assert.equal(snapshot.appId, "task-workbench");
    assert.equal(snapshot.storage.length, 1);

    const dbSnapshot = await host.runControlCommand("db.snapshot", {});
    const persistedSnapshot = dbSnapshot.runtime_snapshots.find((row) => row.snapshot_id === snapshot.snapshotId);
    assert.ok(persistedSnapshot);
    assert.equal(persistedSnapshot.app_id, "task-workbench");
    assert.equal(persistedSnapshot.type, "manual");
    assert.match(persistedSnapshot.content_hash, /^sha256:[a-f0-9]{64}$/);

    const compare = await host.runControlCommand("runtime.compare_snapshot", {
      leftSnapshotId: snapshot.snapshotId,
      rightSnapshotId: snapshot.snapshotId,
    });
    assert.equal(compare.ok, true);
    assert.equal(compare.equal, true);

    const event = { type: "task.created", title: "Replay acceptance" };
    const step = await host.runControlCommand("runtime.core_step", {
      appId: "task-workbench",
      event,
    });
    assert.equal(step.ok, true);
    assert.equal(step.result.actions.some((action) => action.type === "EventAccepted"), true);

    const replay = await host.runControlCommand("runtime.replay_events", {
      appId: "task-workbench",
      events: [event],
    });
    assert.equal(replay.ok, true);
    assert.equal(replay.replay.length, 1);
    assert.equal(replay.replay[0].result.actions.some((action) => action.type === "EventAccepted"), true);

    host.installPackage(path.join(examplesDir, "api-dashboard"));
    const deniedNetwork = await host.runControlCommand("runtime.call_bridge", {
      appId: "api-dashboard",
      method: "network.request",
      params: {
        url: "https://blocked.example.com/status",
        method: "GET",
        headers: {},
        timeoutMs: 10000,
      },
    });
    assert.equal(deniedNetwork.ok, false);
    assert.equal(deniedNetwork.error.code, "network_policy_denied");

    fs.cpSync(path.join(examplesDir, "notes-lite"), budgetDir, { recursive: true });
    const manifestPath = path.join(budgetDir, "manifest.json");
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    manifest.resourceBudget = { ...manifest.resourceBudget, maxStorageBytes: 8 };
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
    host.installPackage(budgetDir);

    const budgetViolation = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "this is too large" }],
    });
    assert.equal(budgetViolation.ok, false);
    assert.equal(budgetViolation.error.code, "resource_budget_exceeded");
    assert.equal(budgetViolation.error.details.budget, "maxStorageBytes");

    const persistedBridgeCalls = await host.runControlCommand("db.query_bridge_calls", { appId: "api-dashboard" });
    const persistedCoreEvents = await host.runControlCommand("db.query_core_events", { appId: "task-workbench" });
    assert.equal(
      persistedBridgeCalls.some((row) => row.method === "network.request"),
      true,
    );
    assert.equal(
      persistedCoreEvents.some((row) => {
        return JSON.parse(row.event_json).title === "Replay acceptance";
      }),
      true,
    );
  } finally {
    host.close();
  }
});
