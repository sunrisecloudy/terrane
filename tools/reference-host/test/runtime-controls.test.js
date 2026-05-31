import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { ReferenceHost } from "../src/reference-host.js";
import { examplesDir } from "../src/paths.js";

test("runtime static snapshot, query, and assertions inspect installed app HTML", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));

    const snapshot = await host.runControlCommand("runtime.snapshot", { appId: "notes-lite" });
    assert.equal(snapshot.title, "Notes Lite");
    assert.equal(snapshot.testIds.includes("new-note-button"), true);

    const query = await host.runControlCommand("runtime.query", {
      appId: "notes-lite",
      testId: "new-note-button",
    });
    assert.equal(query.ok, true);
    assert.equal(query.matches[0].tag, "button");

    const click = await host.runControlCommand("runtime.click", {
      appId: "notes-lite",
      testId: "new-note-button",
    });
    assert.equal(click.ok, true);

    const visible = await host.runControlCommand("runtime.assert_visible", {
      appId: "notes-lite",
      selector: "#new-note",
    });
    assert.equal(visible.ok, true);

    const text = await host.runControlCommand("runtime.assert_text", {
      appId: "notes-lite",
      text: "No notes yet",
    });
    assert.equal(text.ok, true);

    const idle = await host.runControlCommand("runtime.wait_for", { kind: "idle" });
    assert.deepEqual(idle, { ok: true, kind: "idle" });

    const waitSelector = await host.runControlCommand("runtime.wait_for", {
      appId: "notes-lite",
      kind: "selector",
      testId: "new-note-button",
    });
    assert.equal(waitSelector.ok, true);
    assert.equal(waitSelector.kind, "selector");

    const waitText = await host.runControlCommand("runtime.wait_for", {
      appId: "notes-lite",
      kind: "text",
      text: "No notes yet",
    });
    assert.equal(waitText.ok, true);
    assert.equal(waitText.kind, "text");

    await assert.rejects(
      () => host.runControlCommand("runtime.wait_for", { appId: "notes-lite", kind: "selector", testId: "missing-button" }),
      { code: "wait_timeout" },
    );

    await assert.rejects(
      () => host.runControlCommand("runtime.click", { appId: "notes-lite", testId: "missing-button" }),
      /Runtime target was not found/,
    );
  } finally {
    host.close();
  }
});

test("runtime.wait_for observes bridge-call logs", async () => {
  const host = new ReferenceHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:wait-for-bridge",
      value: { ok: true },
    });

    const waited = await host.runControlCommand("runtime.wait_for", {
      appId: "notes-lite",
      kind: "bridge_call",
      method: "storage.set",
    });
    assert.equal(waited.ok, true);
    assert.equal(waited.kind, "bridge_call");
    assert.equal(waited.method, "storage.set");
    assert.equal(waited.count, 1);

    await assert.rejects(
      () => host.runControlCommand("runtime.wait_for", { appId: "notes-lite", kind: "bridge_call", method: "network.request" }),
      { code: "wait_timeout" },
    );
  } finally {
    host.close();
  }
});
