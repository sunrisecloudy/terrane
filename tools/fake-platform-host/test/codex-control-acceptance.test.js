import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

const exampleApps = [
  ["notes-lite", "notes-title"],
  ["task-workbench", "task-workbench-title"],
  ["file-transformer", "file-transformer-title"],
  ["api-dashboard", "api-dashboard-title"],
  ["core-replay-lab", "core-replay-title"],
];

test("Codex control plane installs, opens, drives, mocks, and inspects examples", async () => {
  const host = new FakePlatformHost();
  try {
    for (const [appId, titleTestId] of exampleApps) {
      const packagePath = path.join(examplesDir, appId);
      const validation = await host.runControlCommand("platform.validate_package", { packagePath });
      assert.equal(validation.ok, true, `${appId} validates`);

      const install = await host.runControlCommand("platform.install_webapp_package", { packagePath });
      assert.equal(install.status, "enabled", `${appId} installs`);
      assert.equal(typeof install.installId, "string", `${appId} has installId`);

      const opened = await host.runControlCommand("platform.open_webapp", { appId });
      assert.equal(opened.appId, appId, `${appId} opens`);
      assert.equal(typeof opened.sessionId, "string", `${appId} has sessionId`);

      const visible = await host.runControlCommand("runtime.assert_visible", { appId, testId: titleTestId });
      assert.equal(visible.ok, true, `${appId} title is visible`);

      const screenshot = await host.runControlCommand("runtime.screenshot", { appId, label: `${appId}-codex-acceptance` });
      assert.equal(screenshot.ok, true, `${appId} screenshot summary`);
      assert.equal(screenshot.testIds.includes(titleTestId), true, `${appId} screenshot includes test id`);
    }

    const click = await host.runControlCommand("runtime.click", { appId: "notes-lite", testId: "new-note-button" });
    assert.equal(click.ok, true);
    const type = await host.runControlCommand("runtime.type", {
      appId: "notes-lite",
      testId: "note-title-input",
      text: "Codex acceptance",
    });
    assert.equal(type.ok, true);
    const setValue = await host.runControlCommand("runtime.set_value", {
      appId: "notes-lite",
      testId: "note-body-input",
      value: "Selector controls are reachable.",
    });
    assert.equal(setValue.ok, true);
    const text = await host.runControlCommand("runtime.assert_text", { appId: "notes-lite", text: "Notes Lite" });
    assert.equal(text.ok, true);

    const storageWrite = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Stored through Codex control" }],
    });
    assert.equal(storageWrite.ok, true);

    const appLog = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "app.log",
      params: { level: "info", message: "Codex inspected this log" },
    });
    assert.equal(appLog.ok, true);

    const coreStep = await host.runControlCommand("runtime.core_step", {
      appId: "task-workbench",
      event: { type: "task.created", title: "Inspect core log" },
    });
    assert.equal(coreStep.ok, true);

    const networkMock = await host.runControlCommand("runtime.network_mock_set", {
      appId: "api-dashboard",
      method: "GET",
      urlPattern: "https://api.example.com/status",
      response: {
        status: 200,
        headers: { "content-type": "application/json" },
        bodyText: "{\"ok\":true}",
      },
    });
    assert.equal(networkMock.ok, true);
    const networkResponse = await host.runControlCommand("runtime.call_bridge", {
      appId: "api-dashboard",
      method: "network.request",
      params: {
        url: "https://api.example.com/status",
        method: "GET",
        headers: {},
        timeoutMs: 10000,
      },
    });
    assert.equal(networkResponse.ok, true);
    assert.equal(networkResponse.result.status, 200);

    const dialogMock = await host.runControlCommand("runtime.dialog_mock_set", {
      appId: "file-transformer",
      method: "dialog.openFile",
      response: {
        files: [{ name: "codex.txt", mime: "text/plain", size: 5, text: "hello" }],
        cancelled: false,
      },
    });
    assert.equal(dialogMock.ok, true);
    const dialogResponse = await host.runControlCommand("runtime.call_bridge", {
      appId: "file-transformer",
      method: "dialog.openFile",
      params: { accept: ["text/plain"], multiple: false },
    });
    assert.equal(dialogResponse.ok, true);
    assert.equal(dialogResponse.result.files[0].name, "codex.txt");

    const notification = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "notification.toast",
      params: { level: "success", message: "Codex captured this toast" },
    });
    assert.equal(notification.ok, true);
    const capturedNotifications = await host.runControlCommand("runtime.notification_capture", { appId: "notes-lite" });
    assert.equal(
      capturedNotifications.notifications.some((entry) => entry.message === "Codex captured this toast" && entry.level === "success"),
      true,
    );

    const logs = await host.runControlCommand("runtime.console_logs", { appId: "notes-lite" });
    assert.equal(logs.appId, "notes-lite");
    assert.equal(logs.logs.some((entry) => entry.level === "info" && entry.message === "Codex inspected this log"), true);

    const bridgeCalls = await host.runControlCommand("runtime.bridge_calls", { appId: "api-dashboard" });
    assert.equal(bridgeCalls.some((call) => call.method === "network.request"), true);

    const eventLog = await host.runControlCommand("runtime.event_log", { appId: "task-workbench" });
    assert.equal(eventLog.coreEvents.length, 1);
    assert.equal(JSON.parse(eventLog.coreEvents[0].event_json).type, "task.created");

    const storageRows = await host.runControlCommand("db.query_app_storage", { appId: "notes-lite" });
    assert.equal(storageRows.some((row) => row.key === "notes-lite:notes"), true);

    const coreRows = await host.runControlCommand("db.query_core_events", { appId: "task-workbench" });
    assert.equal(coreRows.length, 1);
    assert.equal(JSON.parse(coreRows[0].event_json).title, "Inspect core log");
  } finally {
    host.close();
  }
});
