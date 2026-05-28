import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("runtime static snapshot, query, and assertions inspect installed app HTML", async () => {
  const host = new FakePlatformHost();
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

    await assert.rejects(
      () => host.runControlCommand("runtime.click", { appId: "notes-lite", testId: "missing-button" }),
      /Runtime target was not found/,
    );
  } finally {
    host.close();
  }
});
