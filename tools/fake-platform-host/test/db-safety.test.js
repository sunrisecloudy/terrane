import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { PlatformError } from "../src/errors.js";
import { FakePlatformHost } from "../src/fake-host.js";
import { examplesDir } from "../src/paths.js";

test("control-plane database tools do not execute arbitrary SQL", async () => {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Safe DB surface" }],
    });

    await assert.rejects(
      () => host.runControlCommand("db.query_sql", { sql: "SELECT * FROM apps" }),
      (error) => error instanceof PlatformError && error.code === "unknown_tool",
    );

    const rows = await host.runControlCommand("db.query_app_storage", {
      appId: "notes-lite",
      sql: "DROP TABLE apps",
    });
    assert.equal(rows.length, 1);
    assert.equal(rows[0].key, "notes-lite:notes");

    const snapshot = await host.runControlCommand("db.snapshot", {
      sql: "DROP TABLE apps",
    });
    assert.equal(snapshot.apps.some((app) => app.id === "notes-lite"), true);
  } finally {
    host.close();
  }
});
