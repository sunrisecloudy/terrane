import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { FakePlatformHost } from "../src/fake-host.js";
import { PlatformDatabase } from "../src/platform-database.js";
import { examplesDir, repoRoot } from "../src/paths.js";

test("checked-in database dbtest fixtures execute against fake-host contracts", async (t) => {
  const fixturesDir = path.join(repoRoot, "tests", "db");
  const required = [
    "sqlite-schema.dbtest.json",
    "postgres-schema.dbtest.json",
    "app-install-transaction.dbtest.json",
    "storage-crud.dbtest.json",
    "runtime-logging.dbtest.json",
    "rollback.dbtest.json",
    "migration-dry-run-apply.dbtest.json",
    "backup-export-import.dbtest.json",
    "corruption-handling.dbtest.json",
  ];
  const files = fs.readdirSync(fixturesDir).filter((fileName) => fileName.endsWith(".dbtest.json")).sort();
  assert.deepEqual(required.filter((fileName) => !files.includes(fileName)), []);

  for (const fileName of files) {
    const fixture = JSON.parse(fs.readFileSync(path.join(fixturesDir, fileName), "utf8"));
    await t.test(fixture.name, async () => {
      await runDbFixture(fixture);
    });
  }
});

async function runDbFixture(fixture) {
  switch (fixture.type) {
    case "schema":
      return runSqliteSchemaFixture(fixture);
    case "schema-static":
      return runPostgresSchemaFixture(fixture);
    case "transaction":
      return runTransactionFixture();
    case "storage":
      return runStorageFixture();
    case "runtime-logging":
      return runRuntimeLoggingFixture();
    case "rollback":
      return runRollbackFixture();
    case "migration":
      return runMigrationFixture();
    case "backup":
      return runBackupFixture();
    case "corruption":
      return runCorruptionFixture();
    default:
      throw new Error(`Unknown dbtest fixture type: ${fixture.type}`);
  }
}

function runSqliteSchemaFixture(fixture) {
  const db = new PlatformDatabase();
  try {
    for (const table of fixture.assertTables ?? []) {
      assert.equal(db.get("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?", table)?.name, table);
    }
  } finally {
    db.close();
  }
}

function runPostgresSchemaFixture(fixture) {
  const sql = (fixture.migrations ?? [])
    .map((migrationPath) => fs.readFileSync(path.join(repoRoot, migrationPath), "utf8"))
    .join("\n");
  assert.match(sql, /\bJSONB\b/);
  for (const table of fixture.assertTables ?? []) {
    assert.match(sql, new RegExp(`CREATE TABLE IF NOT EXISTS ${table}\\b`, "i"), table);
  }
}

function runTransactionFixture() {
  const host = new FakePlatformHost();
  try {
    const install = host.installPackage(path.join(examplesDir, "notes-lite"));
    const app = host.database.get("SELECT active_install_id FROM apps WHERE id = ?", "notes-lite");
    assert.equal(app.active_install_id, install.installId);
    assert.equal(host.database.all("SELECT * FROM app_versions WHERE install_id = ?", install.installId).length, 1);
    assert.equal(host.database.all("SELECT * FROM app_files WHERE install_id = ?", install.installId).length >= 4, true);
    assert.equal(host.database.all("SELECT * FROM app_permissions WHERE install_id = ?", install.installId).length >= 1, true);
    assert.equal(host.database.all("SELECT * FROM app_install_reports WHERE install_id = ?", install.installId).length, 1);
    assert.equal(host.database.all("SELECT * FROM app_installations WHERE install_id = ?", install.installId).length >= 1, true);
  } finally {
    host.close();
  }
}

async function runStorageFixture() {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    const set = await host.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "DB fixture" }],
    });
    assert.equal(set.ok, true);

    const get = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(get.result.value[0].title, "DB fixture");

    const list = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "storage.list",
      params: { prefix: "notes-lite:" },
    });
    assert.deepEqual(list.result.keys, ["notes-lite:notes"]);

    const denied = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "task-workbench:tasks",
      defaultValue: [],
    });
    assert.equal(denied.ok, false);
    assert.equal(denied.error.code, "permission_denied");

    const removed = await host.runControlCommand("runtime.call_bridge", {
      appId: "notes-lite",
      method: "storage.remove",
      params: { key: "notes-lite:notes" },
    });
    assert.equal(removed.ok, true);
    assert.equal(host.database.storageList("notes-lite", "notes-lite:").length, 0);
  } finally {
    host.close();
  }
}

async function runRuntimeLoggingFixture() {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "task-workbench"));
    const opened = await host.runControlCommand("platform.open_webapp", { appId: "task-workbench" });
    assert.match(opened.sessionId, /^session_/);

    const appLog = await host.runControlCommand("runtime.call_bridge", {
      appId: "task-workbench",
      sessionId: opened.sessionId,
      method: "app.log",
      params: { level: "info", message: "DB logging fixture" },
    });
    assert.equal(appLog.ok, true);

    const coreStep = await host.runControlCommand("runtime.core_step", {
      appId: "task-workbench",
      sessionId: opened.sessionId,
      event: { type: "task.created", title: "DB logging fixture" },
    });
    assert.equal(coreStep.ok, true);

    const snapshot = await host.runControlCommand("platform.create_snapshot", {
      appId: "task-workbench",
      sessionId: opened.sessionId,
      type: "post-test",
    });
    assert.match(snapshot.snapshotId, /^snapshot_/);

    const smoke = await host.runControlCommand("runtime.run_smoke_tests", { appId: "task-workbench" });
    assert.equal(smoke.status, "passed");

    const runtimeSessions = host.database.all(
      "SELECT * FROM runtime_sessions WHERE session_id = ? AND active_app_id = ?",
      opened.sessionId,
      "task-workbench",
    );
    assert.equal(runtimeSessions.length, 1);

    const bridgeCalls = host.database.all(
      "SELECT * FROM bridge_calls WHERE app_id = ? ORDER BY created_at",
      "task-workbench",
    );
    assert.equal(bridgeCalls.some((row) => row.method === "app.log" && row.session_id === opened.sessionId), true);
    assert.equal(bridgeCalls.some((row) => row.method === "core.step" && row.session_id === opened.sessionId), true);

    const coreEvents = host.database.all(
      "SELECT * FROM core_events WHERE app_id = ? AND session_id = ?",
      "task-workbench",
      opened.sessionId,
    );
    assert.equal(coreEvents.some((row) => JSON.parse(row.event_json).title === "DB logging fixture"), true);

    const eventIds = new Set(coreEvents.map((row) => row.event_id));
    const coreActions = host.database.all(
      "SELECT * FROM core_actions WHERE app_id = ? AND session_id = ?",
      "task-workbench",
      opened.sessionId,
    );
    assert.equal(coreActions.some((row) => eventIds.has(row.event_id)), true);

    const snapshots = host.database.all(
      "SELECT * FROM runtime_snapshots WHERE app_id = ? AND session_id = ? AND type = ?",
      "task-workbench",
      opened.sessionId,
      "post-test",
    );
    assert.equal(snapshots.length, 1);
    assert.match(snapshots[0].content_hash, /^sha256:[a-f0-9]{64}$/);

    const testRuns = host.database.all(
      "SELECT * FROM test_runs WHERE app_id = ? AND micro_test_id = ?",
      "task-workbench",
      "smoke:task-workbench",
    );
    assert.equal(testRuns.some((row) => row.status === "passed"), true);
  } finally {
    host.close();
  }
}

async function runRollbackFixture() {
  const host = new FakePlatformHost();
  const updatedDir = fs.mkdtempSync(path.join(os.tmpdir(), "dbtest-notes-update-"));
  fs.cpSync(path.join(examplesDir, "notes-lite"), updatedDir, { recursive: true });
  const manifestPath = path.join(updatedDir, "manifest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.version = "0.2.0";
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

  try {
    const first = host.installPackage(path.join(examplesDir, "notes-lite"));
    const second = host.installPackage(updatedDir);
    const rollback = await host.runControlCommand("platform.rollback_webapp", { appId: "notes-lite" });
    assert.equal(rollback.activeInstallId, first.installId);
    assert.equal(rollback.rolledBackInstallId, second.installId);
    assert.equal(host.database.activeInstallId("notes-lite"), first.installId);
    assert.equal(host.database.all("SELECT * FROM app_installations WHERE app_id = ? AND action = 'rollback'", "notes-lite").length, 1);
  } finally {
    host.close();
  }
}

async function runMigrationFixture() {
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
    const applied = await host.runControlCommand("platform.migration_apply", { migration });
    assert.equal(applied.status, "passed");
    assert.equal(host.database.all("SELECT * FROM migration_runs WHERE app_id = ?", "task-workbench").length, 2);
    assert.equal(host.database.all("SELECT * FROM runtime_snapshots WHERE app_id = ?", "task-workbench").length, 2);
  } finally {
    host.close();
  }
}

async function runBackupFixture() {
  const source = new FakePlatformHost();
  const target = new FakePlatformHost();
  try {
    source.installPackage(path.join(examplesDir, "notes-lite"));
    await source.runControlCommand("runtime.storage_set", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      value: [{ title: "Portable note" }],
    });
    const backup = await source.runControlCommand("db.export_backup", {});
    assert.match(backup.contentHash, /^sha256:[a-f0-9]{64}$/);
    const imported = await target.runControlCommand("db.import_backup", { backup });
    assert.equal(imported.ok, true);
    const restored = await target.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:notes",
      defaultValue: [],
    });
    assert.equal(restored.result.value[0].title, "Portable note");
  } finally {
    source.close();
    target.close();
  }
}

async function runCorruptionFixture() {
  const host = new FakePlatformHost();
  try {
    host.installPackage(path.join(examplesDir, "notes-lite"));
    host.database.run(
      "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now'))",
      "notes-lite",
      "notes-lite:corrupt",
      "{",
    );
    const corruptRead = await host.runControlCommand("runtime.storage_get", {
      appId: "notes-lite",
      key: "notes-lite:corrupt",
      defaultValue: null,
    });
    assert.equal(corruptRead.ok, false);
    assert.equal(corruptRead.error.code, "internal_error");
    assert.equal((await host.runControlCommand("platform.health", {})).ok, true);
  } finally {
    host.close();
  }
}
