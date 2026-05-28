import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("fake-host exposes common control utility tools", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));

    const targets = await host.runControlCommand("platform.list_targets", {});
    assert.equal(targets.targets.some((target) => target.id === "fake-host" && target.status === "available"), true);

    const set = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Utility test" }],
    });
    assert.equal(set.ok, true);

    const bridgeAssertion = await host.runControlCommand("runtime.assert_bridge_call", {
      appId: "notes-lite",
      method: "storage.set",
    });
    assert.equal(bridgeAssertion.ok, true);
    assert.equal(bridgeAssertion.count, 1);

    const usage = await host.runControlCommand("runtime.resource_usage", { appId: "notes-lite" });
    assert.equal(usage.storageBytes > 0, true);
    assert.equal(usage.bridgeCallsLastMinute >= 1, true);

    const reset = await host.runControlCommand("platform.reset_webapp", { appId: "notes-lite" });
    assert.equal(reset.ok, true);
    assert.equal(reset.clearedStorageKeys, 1);
    const storage = await host.runControlCommand("db.query_app_storage", { appId: "notes-lite" });
    assert.equal(storage.length, 0);

    const cleared = await host.runControlCommand("runtime.clear_logs", { appId: "notes-lite" });
    assert.equal(cleared.ok, true);
    const calls = await host.runControlCommand("db.query_bridge_calls", { appId: "notes-lite" });
    assert.equal(calls.length, 0);

    const consoleAssertion = await host.runControlCommand("runtime.assert_no_console_errors", { appId: "notes-lite" });
    assert.equal(consoleAssertion.ok, true);
  } finally {
    host.close();
  }
});
