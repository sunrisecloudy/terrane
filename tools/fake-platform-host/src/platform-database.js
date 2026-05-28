import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "node:path";
import { sqliteMigrationsDir } from "./paths.js";
import { canonicalJson, id, nowIso, prettyJson, sha256 } from "./util.js";

export class PlatformDatabase {
  constructor({ dbFile = ":memory:", migrationsDir = sqliteMigrationsDir } = {}) {
    this.dbFile = dbFile;
    this.db = new DatabaseSync(dbFile);
    this.db.exec("PRAGMA foreign_keys = ON");
    this.applyMigrations(migrationsDir);
  }

  applyMigrations(migrationsDir) {
    const migrations = fs
      .readdirSync(migrationsDir)
      .filter((file) => file.endsWith(".sql"))
      .sort();

    for (const migration of migrations) {
      this.db.exec(fs.readFileSync(path.join(migrationsDir, migration), "utf8"));
    }
  }

  close() {
    this.db.close();
  }

  insertInstalledPackage({ manifest, files, hashes, validation, signature, contentHashesDocument, trustLevel = "developer" }) {
    const createdAt = nowIso();
    const installId = `install_${manifest.id}_${manifest.version}_${createdAt.replace(/[-:.]/g, "").slice(0, 15)}_${hashes.contentHash.replace("sha256:", "").slice(0, 12)}_${id("v").slice(2, 10)}`;
    const reportId = id("report");
    const previousInstallId = this.activeInstallId(manifest.id);

    this.transaction(() => {
      this.run(
        "INSERT INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = 'enabled', data_version = excluded.data_version, updated_at = excluded.updated_at",
        manifest.id,
        manifest.name,
        manifest.dataVersion,
        createdAt,
        createdAt,
      );

      if (previousInstallId) {
        this.run("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", previousInstallId);
      }

      this.run(
        "INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'enabled', ?, ?)",
        installId,
        manifest.id,
        manifest.version,
        manifest.runtimeVersion,
        manifest.dataVersion,
        prettyJson(manifest),
        hashes.manifestHash,
        hashes.contentHash,
        prettyJson(signature),
        trustLevel,
        createdAt,
        createdAt,
      );

      for (const [filePath, content] of files.entries()) {
        this.run(
          "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
          installId,
          filePath,
          content,
          hashes.fileHashes[filePath],
          Buffer.byteLength(content),
          mimeForPath(filePath),
          createdAt,
        );
      }

      for (const permission of manifest.permissions) {
        this.run(
          "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) VALUES (?, ?, ?, 1, 1, ?, 'dev install approved')",
          installId,
          manifest.id,
          permission,
          createdAt,
        );
      }

      this.run(
        "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) VALUES (?, ?, ?, 'accepted', ?, ?, ?, ?, ?, ?, ?)",
        reportId,
        manifest.id,
        installId,
        prettyJson(validation),
        prettyJson({ ok: true, signature, contentHashes: contentHashesDocument }),
        prettyJson({ approved: manifest.permissions }),
        prettyJson({ ok: true, runtimeVersion: manifest.runtimeVersion }),
        prettyJson({ status: "not-run" }),
        hashes.contentHash,
        createdAt,
      );

      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, actor, report_id, created_at, details_json) VALUES (?, ?, ?, 'install', 'fake-host', ?, ?, ?)",
        id("install_event"),
        manifest.id,
        installId,
        reportId,
        createdAt,
        prettyJson({ trustLevel }),
      );

      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, actor, report_id, created_at, details_json) VALUES (?, ?, ?, 'activate', 'fake-host', ?, ?, ?)",
        id("install_event"),
        manifest.id,
        installId,
        reportId,
        createdAt,
        prettyJson({ previousInstallId }),
      );

      this.run(
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, updated_at = ? WHERE id = ?",
        installId,
        manifest.version,
        manifest.dataVersion,
        createdAt,
        manifest.id,
      );
    });

    return { installId, reportId, appId: manifest.id, version: manifest.version, contentHash: hashes.contentHash };
  }

  listWebappVersions(appId) {
    return this.all(
      "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at FROM app_versions WHERE app_id = ? ORDER BY created_at DESC",
      appId,
    ).map((row) => ({
      appId: row.app_id,
      appVersion: row.version,
      installId: row.install_id,
      status: row.status,
      installedAt: row.created_at,
      manifestHash: row.manifest_hash,
      contentHash: row.content_hash,
      dataVersion: row.data_version,
      signature: row.signature_json ? JSON.parse(row.signature_json) : null,
      activatedAt: row.activated_at,
      trustLevel: row.trust_level,
      runtimeVersion: row.runtime_version,
    }));
  }

  rollbackWebapp(appId, targetInstallId = null) {
    const active = this.activeInstall(appId);
    if (!active) {
      throw new Error(`App is not installed: ${appId}`);
    }

    const target =
      targetInstallId ??
      this.get(
        "SELECT install_id FROM app_versions WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled') ORDER BY created_at DESC LIMIT 1",
        appId,
        active.installId,
      )?.install_id;

    if (!target) {
      throw new Error(`No rollback target exists for ${appId}`);
    }

    const targetRow = this.get("SELECT version, data_version FROM app_versions WHERE app_id = ? AND install_id = ?", appId, target);
    if (!targetRow) {
      throw new Error(`Rollback target not found: ${target}`);
    }

    const createdAt = nowIso();
    this.transaction(() => {
      this.run("UPDATE app_versions SET status = 'rolled-back' WHERE install_id = ?", active.installId);
      this.run("UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", createdAt, target);
      this.run(
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        target,
        targetRow.version,
        targetRow.data_version,
        createdAt,
        appId,
      );
      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, 'rollback', ?, 'fake-host', ?, ?)",
        id("install_event"),
        appId,
        target,
        active.installId,
        createdAt,
        prettyJson({ targetInstallId: target, rolledBackInstallId: active.installId }),
      );
    });

    return { appId, activeInstallId: target, rolledBackInstallId: active.installId };
  }

  quarantineWebapp(appId, installId = null, reason = "manual quarantine") {
    const active = this.activeInstall(appId);
    const target = installId ?? active?.installId;
    if (!target) {
      throw new Error(`App is not installed: ${appId}`);
    }

    const createdAt = nowIso();
    this.transaction(() => {
      this.run("UPDATE app_versions SET status = 'quarantined' WHERE app_id = ? AND install_id = ?", appId, target);
      if (active?.installId === target) {
        this.run("UPDATE apps SET status = 'quarantined', updated_at = ? WHERE id = ?", createdAt, appId);
      }
      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, actor, created_at, details_json) VALUES (?, ?, ?, 'quarantine', 'fake-host', ?, ?)",
        id("install_event"),
        appId,
        target,
        createdAt,
        prettyJson({ reason }),
      );
    });

    return { appId, installId: target, status: "quarantined", reason };
  }

  installReport(appId, installId = null) {
    const row = installId
      ? this.get("SELECT * FROM app_install_reports WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC LIMIT 1", appId, installId)
      : this.get("SELECT * FROM app_install_reports WHERE app_id = ? ORDER BY created_at DESC LIMIT 1", appId);
    if (!row) return null;
    return {
      reportId: row.report_id,
      appId: row.app_id,
      installId: row.install_id,
      status: row.status,
      validation: row.validation_json ? JSON.parse(row.validation_json) : null,
      security: row.security_json ? JSON.parse(row.security_json) : null,
      permissions: row.permissions_json ? JSON.parse(row.permissions_json) : null,
      compatibility: row.compatibility_json ? JSON.parse(row.compatibility_json) : null,
      smokeTest: row.smoke_test_json ? JSON.parse(row.smoke_test_json) : null,
      contentHash: row.content_hash,
      createdAt: row.created_at,
    };
  }

  createSnapshot({ appId, type = "manual", sessionId = null } = {}) {
    const active = appId ? this.activeInstall(appId) : null;
    const snapshot = {
      appId,
      activeInstallId: active?.installId ?? null,
      activeVersion: active?.version ?? null,
      dataVersion: active?.manifest?.dataVersion ?? null,
      storage: appId ? this.queryAppStorage(appId) : [],
      createdAt: nowIso(),
    };
    const snapshotId = id("snapshot");
    this.run(
      "INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      snapshotId,
      sessionId,
      appId,
      active?.installId ?? null,
      type,
      prettyJson(snapshot),
      `sha256:${sha256(canonicalJson(snapshot))}`,
      snapshot.createdAt,
    );
    return { snapshotId, ...snapshot };
  }

  restoreSnapshot(snapshotId) {
    const row = this.get("SELECT snapshot_json FROM runtime_snapshots WHERE snapshot_id = ?", snapshotId);
    if (!row) {
      throw new Error(`Snapshot not found: ${snapshotId}`);
    }
    const snapshot = JSON.parse(row.snapshot_json);
    this.transaction(() => {
      if (snapshot.appId) {
        this.run("DELETE FROM app_storage WHERE app_id = ?", snapshot.appId);
      }
      for (const item of snapshot.storage ?? []) {
        this.run(
          "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
          item.app_id,
          item.key,
          item.value_json,
          nowIso(),
        );
      }
      if (snapshot.appId && snapshot.activeInstallId) {
        this.run(
          "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
          snapshot.activeInstallId,
          snapshot.activeVersion,
          snapshot.dataVersion,
          nowIso(),
          snapshot.appId,
        );
      }
    });
    return { ok: true, snapshotId, appId: snapshot.appId };
  }

  runMigration({ migration, mode = "dry-run" }) {
    if (!migration || typeof migration !== "object") {
      throw new Error("Migration must be an object");
    }
    if (!["dry-run", "apply"].includes(mode)) {
      throw new Error(`Unsupported migration mode: ${mode}`);
    }
    if (migration.toDataVersion !== migration.fromDataVersion + 1) {
      throw new Error("Migration toDataVersion must equal fromDataVersion + 1");
    }

    const active = this.activeInstall(migration.appId);
    if (!active) {
      throw new Error(`App is not installed: ${migration.appId}`);
    }

    const migrationId = `migration_${migration.appId}_${migration.fromDataVersion}_to_${migration.toDataVersion}`;
    const runId = id("mrun");
    const startedAt = nowIso();
    const preSnapshot = this.createSnapshot({ appId: migration.appId, type: "pre-migration" });
    const preview = this.previewMigration(migration);

    this.run(
      "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      migrationId,
      migration.appId,
      migration.fromDataVersion,
      migration.toDataVersion,
      prettyJson(migration),
      `sha256:${sha256(canonicalJson(migration))}`,
      startedAt,
    );

    if (mode === "dry-run") {
      this.run(
        "INSERT INTO migration_runs (migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, report_json, started_at, finished_at) VALUES (?, ?, ?, ?, 'dry-run', 'passed', ?, ?, ?, ?)",
        runId,
        migrationId,
        migration.appId,
        active.installId,
        preSnapshot.snapshotId,
        prettyJson({ changedKeys: preview.changedKeys, operationCounts: preview.operationCounts }),
        startedAt,
        nowIso(),
      );
      return { runId, mode, status: "passed", snapshotId: preSnapshot.snapshotId, ...preview };
    }

    this.transaction(() => {
      for (const change of preview.changes) {
        if (change.delete) {
          this.run("DELETE FROM app_storage WHERE app_id = ? AND key = ?", migration.appId, change.key);
        } else {
          this.run(
            "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            migration.appId,
            change.key,
            prettyJson(change.value),
            nowIso(),
          );
        }
      }
      this.run("UPDATE apps SET data_version = ?, updated_at = ? WHERE id = ?", migration.toDataVersion, nowIso(), migration.appId);
      this.run(
        "INSERT INTO migration_runs (migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, report_json, started_at, finished_at) VALUES (?, ?, ?, ?, 'apply', 'passed', ?, ?, ?, ?)",
        runId,
        migrationId,
        migration.appId,
        active.installId,
        preSnapshot.snapshotId,
        prettyJson({ changedKeys: preview.changedKeys, operationCounts: preview.operationCounts }),
        startedAt,
        nowIso(),
      );
    });

    return { runId, mode, status: "passed", snapshotId: preSnapshot.snapshotId, ...preview };
  }

  previewMigration(migration) {
    const rows = this.queryAppStorage(migration.appId);
    const values = new Map(rows.map((row) => [row.key, JSON.parse(row.value_json)]));
    const changes = [];
    const operationCounts = {};

    for (const step of migration.steps ?? []) {
      operationCounts[step.op] = (operationCounts[step.op] ?? 0) + 1;
      if (step.op === "setDefault") {
        const key = requiredStepField(step, "key");
        const field = requiredStepField(step, "to");
        const next = setDefault(cloneJson(values.get(key)), field, step.value);
        values.set(key, next);
        changes.push({ key, value: next });
      } else if (step.op === "renameKey" || step.op === "moveStorageKey") {
        const from = requiredStepField(step, "from");
        const to = requiredStepField(step, "to");
        const value = cloneJson(values.get(from));
        values.delete(from);
        values.set(to, value);
        changes.push({ key: from, delete: true });
        changes.push({ key: to, value });
      } else if (step.op === "deleteKey" || step.op === "deleteStorageKey") {
        const key = requiredStepField(step, "key");
        values.delete(key);
        changes.push({ key, delete: true });
      } else if (step.op === "copyKey") {
        const from = requiredStepField(step, "from");
        const to = requiredStepField(step, "to");
        const value = cloneJson(values.get(from));
        values.set(to, value);
        changes.push({ key: to, value });
      } else {
        throw new Error(`Unsupported migration op: ${step.op}`);
      }
    }

    return {
      changedKeys: [...new Set(changes.map((change) => change.key))].sort(),
      operationCounts,
      changes,
    };
  }

  activeInstall(appId) {
    const row = this.get(
      "SELECT apps.id AS app_id, apps.active_install_id, apps.active_version, app_versions.manifest_json, app_versions.signature_json, app_versions.status FROM apps LEFT JOIN app_versions ON app_versions.install_id = apps.active_install_id WHERE apps.id = ?",
      appId,
    );
    if (!row || !row.active_install_id) {
      return null;
    }
    return {
      appId: row.app_id,
      installId: row.active_install_id,
      version: row.active_version,
      manifest: JSON.parse(row.manifest_json),
      signature: row.signature_json ? JSON.parse(row.signature_json) : null,
      status: row.status,
    };
  }

  activeInstallPackage(appId) {
    const active = this.activeInstall(appId);
    if (!active) {
      return null;
    }

    const files = new Map(
      this.all("SELECT path, content_text FROM app_files WHERE install_id = ? ORDER BY path", active.installId).map(
        (row) => [row.path, row.content_text ?? ""],
      ),
    );

    return {
      ...active,
      files,
    };
  }

  activeInstallId(appId) {
    return this.get("SELECT active_install_id FROM apps WHERE id = ?", appId)?.active_install_id ?? null;
  }

  approvedPermissions(appId) {
    const active = this.activeInstall(appId);
    if (!active) {
      return new Set();
    }
    return new Set(
      this.all("SELECT permission FROM app_permissions WHERE install_id = ? AND approved = 1", active.installId).map(
        (row) => row.permission,
      ),
    );
  }

  createRuntimeSession({ appId = null, metadata = {} } = {}) {
    const sessionId = id("session");
    const createdAt = nowIso();
    const active = appId ? this.activeInstall(appId) : null;
    this.run(
      "INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json) VALUES (?, 'fake-host', 'fake-host', '0.1.0', ?, ?, ?, 'running', ?, ?)",
      sessionId,
      appId,
      active?.installId ?? null,
      createdAt,
      prettyJson({ platform: "fake-host" }),
      prettyJson(metadata),
    );
    return sessionId;
  }

  logBridgeCall({ sessionId, appId, installId = null, method, params, result = null, error = null, durationMs = 0 }) {
    this.run(
      "INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      id("bridge"),
      sessionId,
      appId,
      installId,
      method,
      prettyJson(params ?? null),
      result ? prettyJson(result) : null,
      error ? prettyJson(error) : null,
      durationMs,
      nowIso(),
    );
  }

  storageGet(appId, key, defaultValue = null) {
    const row = this.get("SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", appId, key);
    return row ? JSON.parse(row.value_json) : defaultValue;
  }

  storageSet(appId, key, value) {
    const valueJson = prettyJson(value);
    this.run(
      "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
      appId,
      key,
      valueJson,
      nowIso(),
    );
    return Buffer.byteLength(valueJson);
  }

  storageRemove(appId, key) {
    this.run("DELETE FROM app_storage WHERE app_id = ? AND key = ?", appId, key);
  }

  storageList(appId, prefix) {
    return this.all("SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", appId, `${prefix}%`).map(
      (row) => row.key,
    );
  }

  addNetworkMock({ sessionId = null, appId = null, method = "GET", urlPattern, response }) {
    this.run(
      "INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?)",
      id("netmock"),
      sessionId,
      appId,
      method.toUpperCase(),
      urlPattern,
      prettyJson(response),
      nowIso(),
    );
  }

  findNetworkMock({ sessionId = null, appId, method, url }) {
    const rows = this.all(
      "SELECT response_json, url_pattern FROM network_mocks WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) ORDER BY created_at DESC",
      method.toUpperCase(),
      appId,
      sessionId,
    );
    const row = rows.find((candidate) => urlMatches(candidate.url_pattern, url));
    return row ? JSON.parse(row.response_json) : null;
  }

  addDialogMock({ sessionId = null, appId = null, dialogType, response }) {
    this.run(
      "INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at) VALUES (?, ?, ?, ?, ?, 1, ?)",
      id("dialogmock"),
      sessionId,
      appId,
      dialogType,
      prettyJson(response),
      nowIso(),
    );
  }

  findDialogMock({ sessionId = null, appId, dialogType }) {
    const row = this.get(
      "SELECT response_json FROM dialog_mocks WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) ORDER BY created_at DESC LIMIT 1",
      dialogType,
      appId,
      sessionId,
    );
    return row ? JSON.parse(row.response_json) : null;
  }

  snapshot() {
    return {
      apps: this.all("SELECT * FROM apps ORDER BY id"),
      app_versions: this.all("SELECT * FROM app_versions ORDER BY app_id, version"),
      app_storage: this.all("SELECT * FROM app_storage ORDER BY app_id, key"),
      bridge_calls: this.all("SELECT * FROM bridge_calls ORDER BY created_at"),
      runtime_sessions: this.all("SELECT * FROM runtime_sessions ORDER BY started_at"),
      runtime_snapshots: this.all("SELECT * FROM runtime_snapshots ORDER BY created_at"),
      app_migrations: this.all("SELECT * FROM app_migrations ORDER BY created_at"),
      migration_runs: this.all("SELECT * FROM migration_runs ORDER BY started_at"),
      test_runs: this.all("SELECT * FROM test_runs ORDER BY started_at"),
    };
  }

  queryAppStorage(appId) {
    return this.all("SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key", appId);
  }

  queryAppVersions(appId) {
    return this.all(
      "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at FROM app_versions WHERE app_id = ? ORDER BY created_at",
      appId,
    );
  }

  queryBridgeCalls(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM bridge_calls WHERE app_id = ? ORDER BY created_at", appId);
    }
    return this.all("SELECT * FROM bridge_calls ORDER BY created_at");
  }

  queryCoreEvents(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM core_events WHERE app_id = ? ORDER BY created_at", appId);
    }
    return this.all("SELECT * FROM core_events ORDER BY created_at");
  }

  queryTestRuns(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM test_runs WHERE app_id = ? ORDER BY started_at", appId);
    }
    return this.all("SELECT * FROM test_runs ORDER BY started_at");
  }

  run(sql, ...params) {
    return this.db.prepare(sql).run(...params);
  }

  get(sql, ...params) {
    return this.db.prepare(sql).get(...params);
  }

  all(sql, ...params) {
    return this.db.prepare(sql).all(...params);
  }

  transaction(fn) {
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const result = fn();
      this.db.exec("COMMIT");
      return result;
    } catch (error) {
      this.db.exec("ROLLBACK");
      throw error;
    }
  }
}

function mimeForPath(filePath) {
  if (filePath.endsWith(".html")) return "text/html";
  if (filePath.endsWith(".css")) return "text/css";
  if (filePath.endsWith(".js")) return "text/javascript";
  if (filePath.endsWith(".json")) return "application/json";
  return "text/plain";
}

function urlMatches(pattern, url) {
  if (pattern === url) return true;
  if (pattern.endsWith("*")) return url.startsWith(pattern.slice(0, -1));
  return false;
}

function requiredStepField(step, field) {
  if (!(field in step)) {
    throw new Error(`Migration step ${step.op} requires ${field}`);
  }
  return step[field];
}

function cloneJson(value) {
  return value === undefined ? null : JSON.parse(JSON.stringify(value));
}

function setDefault(value, field, defaultValue) {
  if (Array.isArray(value)) {
    return value.map((item) => setDefault(item, field, defaultValue));
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    if (!(field in value)) {
      return { ...value, [field]: defaultValue };
    }
    return value;
  }
  return value;
}
