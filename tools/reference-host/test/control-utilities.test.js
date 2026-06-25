import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import { TOOL_NAMES } from "../../codex-platform-mcp/src/tool-contract.js";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

// Load the renderer's canonical snapshot contract (the section keys + the row
// collections it reads) by executing the browser IIFE against a minimal window.
function loadEngineRoomContract() {
  const source = fs.readFileSync(
    path.join(path.dirname(fileURLToPath(import.meta.url)), "../../../runtime-web/engine-room.js"),
    "utf8",
  );
  const sandbox = { window: {} };
  vm.runInNewContext(source, sandbox);
  return sandbox.window.TerraneEngineRoom;
}

test("reference-host command switch covers every MCP tool name", () => {
  const source = fs.readFileSync(new URL("../src/reference-host.js", import.meta.url), "utf8");
  const implemented = new Set([...source.matchAll(/case "([^"]+)":/g)].map((match) => match[1]));
  const missing = TOOL_NAMES.filter((toolName) => !implemented.has(toolName));

  assert.deepEqual(missing, []);
});

test("reference-host exposes common control utility tools", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    host.installPackage(path.join(examplesDir, "task-workbench"));

    const targets = await host.runControlCommand("platform.list_targets", {});
    assert.equal(targets.targets.some((target) => target.id === "reference-host" && target.status === "available"), true);

    const launch = await host.runControlCommand("platform.launch", { port: 9191 });
    assert.equal(launch.ok, true);
    assert.equal(launch.status, "running");
    assert.equal(launch.url, "http://127.0.0.1:9191");

    const stopped = await host.runControlCommand("platform.stop", {});
    assert.equal(stopped.ok, true);
    assert.equal(stopped.status, "stopped");

    const reload = await host.runControlCommand("platform.reload_runtime", {});
    assert.deepEqual(reload, { ok: true, target: "reference-host", status: "reloaded" });

    const webapps = await host.runControlCommand("platform.list_webapps", {});
    assert.equal(webapps.apps.some((app) => app.appId === "notes-lite" && app.installed === true), true);
    assert.equal(webapps.apps.some((app) => app.appId === "api-dashboard" && app.bundled === true), true);

    const drag = await host.runControlCommand("runtime.drag", {
      appId: "notes-lite",
      testId: "new-note-button",
    });
    assert.equal(drag.ok, true);

    const set = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Utility test" }],
    });
    assert.equal(set.ok, true);

    const storageAssertion = await host.runControlCommand("runtime.assert_storage", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Utility test" }],
    });
    assert.equal(storageAssertion.ok, true);

    const toast = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "notification.toast",
      params: { message: "Saved" },
    });
    assert.equal(toast.ok, true);
    const notifications = await host.runControlCommand("runtime.notification_capture", { appId: "notes-lite" });
    assert.equal(notifications.notifications.some((notification) => notification.message === "Saved" && notification.bridgeCallId), true);

    const bridgeAssertion = await host.runControlCommand("runtime.assert_bridge_call", {
      appId: "notes-lite",
      method: "storage.set",
    });
    assert.equal(bridgeAssertion.ok, true);
    assert.equal(bridgeAssertion.count, 1);

    const usage = await host.runControlCommand("runtime.resource_usage", { appId: "notes-lite" });
    assert.equal(usage.storageBytes > 0, true);
    assert.equal(usage.bridgeCallsLastMinute >= 1, true);

    const advanced = await host.runControlCommand("runtime.timer_advance", { ms: 250 });
    assert.deepEqual(advanced, { ok: true, advancedMs: 250 });

    const fault = await host.runControlCommand("runtime.fault_inject", {
      appId: "notes-lite",
      kind: "storage.write",
      code: "storage.injected",
      message: "Injected storage write failure",
    });
    assert.equal(fault.ok, true);
    const failedWrite = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Should fail" }],
    });
    assert.equal(failedWrite.ok, false);
    assert.equal(failedWrite.error.code, "storage.injected");
    const recoveredWrite = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Utility test" }],
    });
    assert.equal(recoveredWrite.ok, true);

    const networkMock = await host.runControlCommand("runtime.network_mock_set", {
      appId: "notes-lite",
      method: "GET",
      urlPattern: "https://example.test/*",
      response: { status: 200, body: "ok" },
    });
    assert.equal(networkMock.ok, true);
    const resetNetworkMocks = await host.runControlCommand("runtime.network_mock_reset", { appId: "notes-lite" });
    assert.equal(resetNetworkMocks.cleared, 1);

    const coreStep = await host.runControlCommand("runtime.core_step", {
      appId: "task-workbench",
      event: { type: "task.created", title: "Parity check" },
    });
    assert.equal(coreStep.ok, true);
    assert.equal(coreStep.result.stateVersion, 1);

    const coreSnapshot = await host.runControlCommand("runtime.core_snapshot", { appId: "task-workbench" });
    assert.deepEqual(coreSnapshot, { appId: "task-workbench", stateVersion: 1 });

    const replay = await host.runControlCommand("runtime.replay_events", {
      appId: "task-workbench",
      events: [{ type: "task.created", title: "Replay check" }],
    });
    assert.equal(replay.ok, true);
    assert.equal(replay.replay.length, 1);
    assert.equal(replay.replay[0].result.stateVersion, 1);

    const eventLog = await host.runControlCommand("runtime.event_log", { appId: "task-workbench" });
    assert.equal(eventLog.coreEvents.length, 1);
    assert.equal(JSON.parse(eventLog.coreEvents[0].event_json).type, "task.created");

    const coreAction = await host.runControlCommand("runtime.assert_core_action", {
      appId: "task-workbench",
      type: "Log",
      match: { message: "Unhandled event: task.created" },
    });
    assert.equal(coreAction.ok, true);

    const consoleLogs = await host.runControlCommand("runtime.console_logs", { appId: "task-workbench" });
    assert.deepEqual(consoleLogs, { appId: "task-workbench", logs: [] });

    const equalCompare = await host.runControlCommand("runtime.compare_snapshot", {
      left: { b: 2, a: 1 },
      right: { a: 1, b: 2 },
    });
    assert.equal(equalCompare.ok, true);
    assert.equal(equalCompare.equal, true);
    assert.equal(equalCompare.leftHash, equalCompare.rightHash);

    const differentCompare = await host.runControlCommand("runtime.compare_snapshot", {
      left: { value: 1 },
      right: { value: 2 },
    });
    assert.equal(differentCompare.ok, false);
    assert.equal(differentCompare.equal, false);
    assert.notEqual(differentCompare.leftHash, differentCompare.rightHash);

    const snapshot = await host.runControlCommand("platform.create_snapshot", { appId: "notes-lite" });
    const snapshotCompare = await host.runControlCommand("runtime.compare_snapshot", {
      leftSnapshotId: snapshot.snapshotId,
      rightSnapshotId: snapshot.snapshotId,
    });
    assert.equal(snapshotCompare.ok, true);

    const repair = await host.runControlCommand("platform.run_repair_loop", {
      packagePath: path.join(examplesDir, "notes-lite"),
    });
    assert.equal(repair.ok, true);
    assert.equal(repair.finalStatus, "passed");
    assert.equal(repair.attempts, 1);
    assert.equal(repair.snapshots.length, 1);
    assert.equal(repair.testsRun.includes("smoke:notes-lite"), true);

    await assert.rejects(
      () => host.runControlCommand("platform.uninstall_webapp", { appId: "task-workbench" }),
      /requires confirm/,
    );
    const uninstall = await host.runControlCommand("platform.uninstall_webapp", { appId: "task-workbench", confirm: true });
    assert.equal(uninstall.status, "uninstalled");
    const withUninstalled = await host.runControlCommand("platform.list_webapps", { includeUninstalled: true });
    assert.equal(withUninstalled.apps.some((app) => app.appId === "task-workbench" && app.status === "uninstalled"), true);

    await assert.rejects(
      () => host.runControlCommand("platform.reset_webapp", { appId: "notes-lite" }),
      /requires confirm/,
    );
    await assert.rejects(
      () => host.runControlCommand("runtime.storage_reset", { appId: "notes-lite" }),
      /requires confirm/,
    );
    const reset = await host.runControlCommand("platform.reset_webapp", { appId: "notes-lite", confirm: true });
    assert.equal(reset.ok, true);
    assert.equal(reset.clearedStorageKeys, 1);
    const storage = await host.runControlCommand("db.query_app_storage", { appId: "notes-lite" });
    assert.equal(storage.length, 0);

    const cleared = await host.runControlCommand("runtime.clear_logs", { appId: "notes-lite" });
    assert.equal(cleared.ok, true);
    const calls = await host.runControlCommand("db.query_bridge_calls", { appId: "notes-lite" });
    assert.equal(calls.length, 0);
    const clearedNotifications = await host.runControlCommand("runtime.notification_capture", { appId: "notes-lite" });
    assert.deepEqual(clearedNotifications, { appId: "notes-lite", notifications: [] });

    const consoleAssertion = await host.runControlCommand("runtime.assert_no_console_errors", { appId: "notes-lite" });
    assert.equal(consoleAssertion.ok, true);
  } finally {
    host.close();
  }
});

test("reference-host Engine Room snapshot groups read-only app and platform data", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });
    await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:engine-room",
      value: { ok: true },
    });
    await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "app.log",
      params: { level: "info", message: "Engine Room probe" },
    });

    const snapshot = await host.runControlCommand("engineRoom.snapshot", { appId: "notes-lite" });

    assert.equal(snapshot.overview.source, "reference-host");
    assert.equal(snapshot.overview.appId, "notes-lite");
    assert.equal(snapshot.apps.rows.some((row) => row.id === "notes-lite"), true);
    assert.equal(snapshot.storage.rows.some((row) => row.key === "notes-lite:engine-room"), true);
    assert.equal(snapshot.bridgeCalls.rows.some((row) => row.method === "storage.set"), true);
    assert.equal(snapshot.logs.appLogRows.some((row) => row.message === "Engine Room probe"), true);
    assert.equal(snapshot.database.tableCounts.app_storage >= 1, true);
    assert.deepEqual(snapshot.sync.server, { status: "not-attached" });

    await assert.rejects(
      () => host.runControlCommand("engineRoom.reset", { appId: "notes-lite" }),
      /Unknown control tool/,
    );
  } finally {
    host.close();
  }
});

test("reference-host Engine Room snapshot conforms to the renderer contract", async () => {
  const contract = loadEngineRoomContract();
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("platform.open_webapp", { appId: "notes-lite" });

    const snapshot = await host.runControlCommand("engineRoom.snapshot", {});

    // Every section the renderer knows how to draw must exist in the host snapshot.
    for (const key of contract.sectionKeys) {
      assert.equal(key in snapshot, true, `host snapshot is missing section "${key}"`);
    }

    // Every row collection the renderer resolves must be present under at least
    // one of its candidate field names (or be the synthetic table-counts view),
    // so the host shape and the renderer stay in lockstep.
    for (const [sectionKey, collections] of Object.entries(contract.collections)) {
      const section = snapshot[sectionKey];
      assert.equal(typeof section, "object", `section "${sectionKey}" must be an object`);
      for (const collection of collections) {
        if (collection.optional) continue;
        if (collection.fields[0] === "__tableCounts") {
          assert.equal(typeof section.tableCounts, "object", `section "${sectionKey}" must expose tableCounts`);
          continue;
        }
        const resolved = collection.fields.some((field) => Array.isArray(section[field]));
        assert.equal(resolved, true, `section "${sectionKey}" collection "${collection.label}" resolved no array field`);
      }
    }
  } finally {
    host.close();
  }
});

test("reference-host exposes app.log rows as console logs and asserts error logs", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));

    const missingMessage = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "app.log",
      params: { level: "info" },
    });
    assert.equal(missingMessage.ok, false);
    assert.equal(missingMessage.error.code, "invalid_request");
    await host.runControlCommand("runtime.clear_logs", { appId: "notes-lite" });

    const info = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "app.log",
      params: { level: "info", message: "utility log" },
    });
    assert.equal(info.ok, true);

    const logs = await host.runControlCommand("runtime.console_logs", { appId: "notes-lite" });
    assert.equal(logs.appId, "notes-lite");
    assert.equal(logs.logs.some((entry) => entry.level === "info" && entry.message === "utility log"), true);
    assert.deepEqual(await host.runControlCommand("runtime.assert_no_console_errors", { appId: "notes-lite" }), { ok: true, errors: 0 });

    const error = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "app.log",
      params: { level: "error", message: "visible failure" },
    });
    assert.equal(error.ok, true);
    await assert.rejects(
      () => host.runControlCommand("runtime.assert_no_console_errors", { appId: "notes-lite" }),
      /Console error logs were found/,
    );

    await host.runControlCommand("runtime.clear_logs", { appId: "notes-lite" });
    assert.deepEqual(await host.runControlCommand("runtime.console_logs", { appId: "notes-lite" }), { appId: "notes-lite", logs: [] });
  } finally {
    host.close();
  }
});
