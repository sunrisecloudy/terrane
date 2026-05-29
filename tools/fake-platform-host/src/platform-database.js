import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "node:path";
import { PlatformError } from "./errors.js";
import { sqliteMigrationsDir } from "./paths.js";
import { canonicalJson, id, nowIso, prettyJson, sha256 } from "./util.js";

export class PlatformDatabase {
  constructor({ dbFile = ":memory:", migrationsDir = sqliteMigrationsDir } = {}) {
    this.dbFile = dbFile;
    this.db = new DatabaseSync(dbFile);
    this.db.exec("PRAGMA foreign_keys = ON");
    this.applyMigrations(migrationsDir);
    this.ensureControlCommandAuditColumns();
    this.ensureRuntimeSessionColumns();
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

  ensureControlCommandAuditColumns() {
    const columns = new Set(this.all("PRAGMA table_info(control_commands)").map((row) => row.name));
    for (const [name, type] of [
      ["http_method", "TEXT"],
      ["path", "TEXT"],
      ["decision", "TEXT"],
      ["error_code", "TEXT"],
    ]) {
      if (!columns.has(name)) {
        this.db.exec(`ALTER TABLE control_commands ADD COLUMN ${name} ${type}`);
      }
    }
  }

  ensureRuntimeSessionColumns() {
    const columns = new Set(this.all("PRAGMA table_info(runtime_sessions)").map((row) => row.name));
    if (!columns.has("resource_high_water_json")) {
      this.db.exec("ALTER TABLE runtime_sessions ADD COLUMN resource_high_water_json TEXT");
    }
  }

  close() {
    this.db.close();
  }

  insertInstalledPackage({
    manifest,
    files,
    hashes,
    validation,
    signature,
    contentHashesDocument,
    trustLevel = "developer",
    smokeTest = { status: "not-run" },
    accessibility = null,
    compatibility = { ok: true },
    approval = { requiresUserApproval: false, reasons: [] },
    activate = true,
    versionStatus = activate ? "enabled" : "quarantined",
    reportStatus = activate ? "accepted" : "failed",
  }) {
    const createdAt = nowIso();
    const installId = `install_${manifest.id}_${manifest.version}_${createdAt.replace(/[-:.]/g, "").slice(0, 15)}_${hashes.contentHash.replace("sha256:", "").slice(0, 12)}_${id("v").slice(2, 10)}`;
    const reportId = id("report");
    const previousInstallId = this.activeInstallId(manifest.id);
    const existingApp = this.get("SELECT id, status, data_version FROM apps WHERE id = ?", manifest.id);
    const appStatus = activate || previousInstallId ? "enabled" : "quarantined";

    this.transaction(() => {
      this.run(
        "INSERT INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status, data_version = excluded.data_version, updated_at = excluded.updated_at",
        manifest.id,
        manifest.name,
        existingApp && !activate ? existingApp.status : appStatus,
        activate || !existingApp ? manifest.dataVersion : existingApp.data_version,
        createdAt,
        createdAt,
      );

      if (previousInstallId && activate) {
        this.run("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", previousInstallId);
      }

      this.run(
        "INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        versionStatus,
        createdAt,
        activate ? createdAt : null,
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
          "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) VALUES (?, ?, ?, 1, ?, ?, ?)",
          installId,
          manifest.id,
          permission,
          activate ? 1 : 0,
          activate ? createdAt : null,
          activate ? "dev install approved" : "pending until quarantined version is repaired",
        );
      }

      this.run(
        "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        reportId,
        manifest.id,
        installId,
        reportStatus,
        prettyJson(validation),
        prettyJson({ ok: accessibility?.status !== "fail", signature, contentHashes: contentHashesDocument, accessibility }),
        prettyJson({
          approved: activate ? manifest.permissions : [],
          requested: manifest.permissions,
          requiresUserApproval: approval.requiresUserApproval === true,
          approvalReasons: approval.reasons ?? [],
          previousInstallId: approval.previousInstallId ?? null,
        }),
        prettyJson(compatibility),
        prettyJson(smokeTest),
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
        prettyJson({ trustLevel, status: versionStatus }),
      );

      if (activate) {
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
          "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
          installId,
          manifest.version,
          manifest.dataVersion,
          createdAt,
          manifest.id,
        );
      } else if (versionStatus === "quarantined") {
        this.run(
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, actor, report_id, created_at, details_json) VALUES (?, ?, ?, 'quarantine', 'fake-host', ?, ?, ?)",
          id("install_event"),
          manifest.id,
          installId,
          reportId,
          createdAt,
          prettyJson({ previousInstallId, reason: "install gate failed" }),
        );
      }
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

  approveWebappUpdate(appId, installId) {
    const target = this.installedPackageByInstallId(installId);
    if (!target || target.appId !== appId) {
      throw new Error(`Install not found for app: ${appId}`);
    }
    if (target.status === "quarantined" || target.status === "uninstalled") {
      throw new Error(`Install cannot be approved from status: ${target.status}`);
    }
    const report = this.installReport(appId, installId);
    if (!report || report.status !== "requires-approval") {
      throw new Error(`Install does not require approval: ${installId}`);
    }

    const active = this.activeInstall(appId);
    const createdAt = nowIso();
    const migrationRuns = this.applyPendingInstallMigrations({ active, target });
    this.transaction(() => {
      if (active?.installId && active.installId !== installId) {
        this.run("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", active.installId);
      }
      this.run("UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", createdAt, installId);
      this.run(
        "UPDATE app_permissions SET approved = 1, approved_at = ?, reason = 'approved update' WHERE install_id = ?",
        createdAt,
        installId,
      );
      this.run(
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        installId,
        target.version,
        target.manifest.dataVersion,
        createdAt,
        appId,
      );
      this.run(
        "UPDATE app_install_reports SET status = 'accepted', permissions_json = ? WHERE report_id = ?",
        prettyJson({
          ...(report.permissions ?? {}),
          approved: target.manifest.permissions,
          requiresUserApproval: true,
          approvalGranted: true,
          approvedAt: createdAt,
        }),
        report.reportId,
      );
      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) VALUES (?, ?, ?, 'activate', ?, 'fake-host', ?, ?, ?)",
        id("install_event"),
        appId,
        installId,
        active?.installId ?? null,
        report.reportId,
        createdAt,
        prettyJson({ approved: true, previousInstallId: active?.installId ?? null, migrationRuns }),
      );
    });

    return { appId, installId, status: "enabled", previousInstallId: active?.installId ?? null, migrationRuns };
  }

  applyPendingInstallMigrations({ active, target }) {
    if (!active || target.manifest.dataVersion <= active.manifest.dataVersion) {
      return [];
    }
    const runs = [];
    for (let from = active.manifest.dataVersion; from < target.manifest.dataVersion; from += 1) {
      const path = `migrations/${from}_to_${from + 1}.json`;
      const content = target.files.get(path);
      if (!content) {
        throw new Error(`Missing migration file: ${path}`);
      }
      const migration = JSON.parse(content);
      runs.push(this.runMigration({ migration, mode: "apply" }));
    }
    return runs;
  }

  quarantineWebapp(appId, installId = null, reason = "manual quarantine", { restorePrevious = false, actor = "fake-host" } = {}) {
    const active = this.activeInstall(appId);
    const target = installId ?? active?.installId;
    if (!target) {
      throw new Error(`App is not installed: ${appId}`);
    }
    const restoreTarget = restorePrevious && active?.installId === target
      ? this.get(
        "SELECT install_id, version, data_version FROM app_versions WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled') ORDER BY created_at DESC LIMIT 1",
        appId,
        target,
      )
      : null;

    const createdAt = nowIso();
    this.transaction(() => {
      this.run("UPDATE app_versions SET status = 'quarantined' WHERE app_id = ? AND install_id = ?", appId, target);
      if (restoreTarget) {
        this.run("UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", createdAt, restoreTarget.install_id);
        this.run(
          "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
          restoreTarget.install_id,
          restoreTarget.version,
          restoreTarget.data_version,
          createdAt,
          appId,
        );
      } else if (active?.installId === target) {
        this.run("UPDATE apps SET status = 'quarantined', updated_at = ? WHERE id = ?", createdAt, appId);
      }
      this.run(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, 'quarantine', ?, ?, ?, ?)",
        id("install_event"),
        appId,
        target,
        restoreTarget?.install_id ?? null,
        actor,
        createdAt,
        prettyJson({ reason, restoredInstallId: restoreTarget?.install_id ?? null }),
      );
      if (restoreTarget) {
        this.run(
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, 'rollback', ?, ?, ?, ?)",
          id("install_event"),
          appId,
          restoreTarget.install_id,
          target,
          actor,
          createdAt,
          prettyJson({ reason: "automatic rollback after quarantine", quarantinedInstallId: target }),
        );
      }
    });

    return { appId, installId: target, status: "quarantined", reason, restoredInstallId: restoreTarget?.install_id ?? null };
  }

  installReport(appId, installId = null) {
    const row = installId
      ? this.get("SELECT * FROM app_install_reports WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC LIMIT 1", appId, installId)
      : this.get("SELECT * FROM app_install_reports WHERE app_id = ? ORDER BY created_at DESC LIMIT 1", appId);
    if (!row) return null;
    const permissions = row.permissions_json ? JSON.parse(row.permissions_json) : null;
    return {
      reportId: row.report_id,
      appId: row.app_id,
      installId: row.install_id,
      status: row.status,
      validation: row.validation_json ? JSON.parse(row.validation_json) : null,
      security: row.security_json ? JSON.parse(row.security_json) : null,
      permissions,
      requiresUserApproval: permissions?.requiresUserApproval === true,
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

  resetWebapp(appId) {
    const snapshot = this.createSnapshot({ appId, type: "manual" });
    this.run("DELETE FROM app_storage WHERE app_id = ?", appId);
    return {
      ok: true,
      appId,
      snapshotId: snapshot.snapshotId,
      clearedStorageKeys: snapshot.storage.length,
    };
  }

  clearRuntimeLogs(appId = null) {
    const bridge = appId
      ? this.run("DELETE FROM bridge_calls WHERE app_id = ?", appId).changes
      : this.run("DELETE FROM bridge_calls").changes;
    const actions = appId
      ? this.run("DELETE FROM core_actions WHERE app_id = ?", appId).changes
      : this.run("DELETE FROM core_actions").changes;
    const events = appId
      ? this.run("DELETE FROM core_events WHERE app_id = ?", appId).changes
      : this.run("DELETE FROM core_events").changes;
    return { ok: true, appId, bridgeCallsCleared: bridge, coreActionsCleared: actions, coreEventsCleared: events };
  }

  resourceUsage(appId) {
    const since = new Date(Date.now() - 60_000).toISOString();
    return {
      appId,
      storageBytes: this.get(
        "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) AS bytes FROM app_storage WHERE app_id = ?",
        appId,
      )?.bytes ?? 0,
      bridgeCallsLastMinute: this.countBridgeCallsSince({ appId, since }),
      networkRequestsLastMinute: this.countBridgeCallsSince({ appId, since, method: "network.request" }),
      logLinesLastMinute: this.countBridgeCallsSince({ appId, since, method: "app.log" }),
    };
  }

  recordResourceHighWater({ sessionId, appId }) {
    if (!sessionId || !appId) return null;
    const row = this.get("SELECT resource_high_water_json FROM runtime_sessions WHERE session_id = ?", sessionId);
    if (!row) return null;
    const current = this.resourceUsage(appId);
    const previous = row.resource_high_water_json ? JSON.parse(row.resource_high_water_json) : {};
    const highWater = {
      appId,
      storageBytes: Math.max(previous.storageBytes ?? 0, current.storageBytes),
      bridgeCallsLastMinute: Math.max(previous.bridgeCallsLastMinute ?? 0, current.bridgeCallsLastMinute),
      networkRequestsLastMinute: Math.max(previous.networkRequestsLastMinute ?? 0, current.networkRequestsLastMinute),
      logLinesLastMinute: Math.max(previous.logLinesLastMinute ?? 0, current.logLinesLastMinute),
      updatedAt: nowIso(),
    };
    this.run("UPDATE runtime_sessions SET resource_high_water_json = ? WHERE session_id = ?", prettyJson(highWater), sessionId);
    return highWater;
  }

  assertBridgeCall({ appId, method }) {
    const rows = this.queryBridgeCalls(appId).filter((row) => row.method === method);
    if (rows.length === 0) {
      throw new PlatformError("assertion_failed", "Expected bridge call was not recorded", { appId, method });
    }
    return { ok: true, appId, method, count: rows.length, latest: rows.at(-1) };
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

  runtimeSnapshotById(snapshotId) {
    const row = this.get("SELECT snapshot_id, snapshot_json, content_hash, created_at FROM runtime_snapshots WHERE snapshot_id = ?", snapshotId);
    if (!row) {
      throw new Error(`Snapshot not found: ${snapshotId}`);
    }
    return {
      snapshotId: row.snapshot_id,
      snapshot: JSON.parse(row.snapshot_json),
      contentHash: row.content_hash,
      createdAt: row.created_at,
    };
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

  installedPackageByInstallId(installId) {
    const row = this.get(
      "SELECT install_id, app_id, version, manifest_json, signature_json, trust_level, status FROM app_versions WHERE install_id = ?",
      installId,
    );
    if (!row) return null;
    return {
      installId: row.install_id,
      appId: row.app_id,
      version: row.version,
      manifest: JSON.parse(row.manifest_json),
      signature: row.signature_json ? JSON.parse(row.signature_json) : null,
      trustLevel: row.trust_level,
      status: row.status,
      files: new Map(
        this.all("SELECT path, content_text FROM app_files WHERE install_id = ? ORDER BY path", installId).map(
          (file) => [file.path, file.content_text ?? ""],
        ),
      ),
    };
  }

  updateInstalledSignature({ installId, signature, hashes }) {
    this.run(
      "UPDATE app_versions SET manifest_hash = ?, content_hash = ?, signature_json = ? WHERE install_id = ?",
      hashes.manifestHash,
      hashes.contentHash,
      prettyJson(signature),
      installId,
    );
    for (const [filePath, contentHash] of Object.entries(hashes.fileHashes)) {
      this.run("UPDATE app_files SET content_hash = ? WHERE install_id = ? AND path = ?", contentHash, installId, filePath);
    }
  }

  activeInstallId(appId) {
    return this.get("SELECT active_install_id FROM apps WHERE id = ?", appId)?.active_install_id ?? null;
  }

  listWebapps({ includeUninstalled = false } = {}) {
    const sql = [
      "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version, a.created_at, a.updated_at, v.runtime_version, v.trust_level",
      "FROM apps a LEFT JOIN app_versions v ON v.install_id = a.active_install_id",
      includeUninstalled ? "" : "WHERE a.status <> 'uninstalled'",
      "ORDER BY a.id",
    ]
      .filter(Boolean)
      .join(" ");
    return this.all(sql).map((row) => ({
      appId: row.id,
      name: row.name,
      status: row.status,
      activeInstallId: row.active_install_id,
      activeVersion: row.active_version,
      dataVersion: row.data_version,
      runtimeVersion: row.runtime_version,
      trustLevel: row.trust_level,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    }));
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
      "INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) VALUES (?, 'fake-host', 'fake-host', '0.1.0', ?, ?, ?, 'running', ?, ?, ?)",
      sessionId,
      appId,
      active?.installId ?? null,
      createdAt,
      prettyJson({ platform: "fake-host" }),
      prettyJson(emptyResourceHighWater(appId)),
      prettyJson(metadata),
    );
    return sessionId;
  }

  createControlSession({ target = "fake-host", appId = null, actor = "codex", metadata = {}, tokenHash = null } = {}) {
    const controlSessionId = id("control");
    const runtimeSessionId = appId ? this.createRuntimeSession({ appId, metadata: { controlSessionId, ...metadata } }) : null;
    const startedAt = nowIso();
    this.run(
      "INSERT INTO control_sessions (control_session_id, target, runtime_session_id, actor, token_hash, started_at, status, metadata_json) VALUES (?, ?, ?, ?, ?, ?, 'running', ?)",
      controlSessionId,
      target,
      runtimeSessionId,
      actor,
      tokenHash,
      startedAt,
      prettyJson({ appId, ...metadata }),
    );
    return { controlSessionId, runtimeSessionId, target, appId, status: "running", startedAt };
  }

  controlSession(controlSessionId) {
    const row = this.get(
      "SELECT c.control_session_id, c.target, c.runtime_session_id, c.actor, c.started_at, c.ended_at, c.status, c.metadata_json, r.active_app_id FROM control_sessions c LEFT JOIN runtime_sessions r ON r.session_id = c.runtime_session_id WHERE c.control_session_id = ?",
      controlSessionId,
    );
    if (!row) {
      throw new Error(`Control session not found: ${controlSessionId}`);
    }
    const metadata = row.metadata_json ? JSON.parse(row.metadata_json) : {};
    return {
      controlSessionId: row.control_session_id,
      target: row.target,
      runtimeSessionId: row.runtime_session_id,
      actor: row.actor,
      appId: row.active_app_id ?? metadata.appId ?? null,
      status: row.status,
      startedAt: row.started_at,
      endedAt: row.ended_at,
      metadata,
    };
  }

  endControlSession(controlSessionId) {
    const endedAt = nowIso();
    const changes = this.run(
      "UPDATE control_sessions SET status = 'ended', ended_at = ? WHERE control_session_id = ?",
      endedAt,
      controlSessionId,
    ).changes;
    if (changes === 0) {
      throw new Error(`Control session not found: ${controlSessionId}`);
    }
    return { ok: true, controlSessionId, status: "ended", endedAt };
  }

  uninstallWebapp(appId, { confirm = false, actor = "codex" } = {}) {
    if (confirm !== true) {
      throw new PlatformError("confirmation_required", "platform.uninstall_webapp requires confirm: true", { appId });
    }
    const app = this.get("SELECT id, active_install_id FROM apps WHERE id = ?", appId);
    if (!app) {
      throw new PlatformError("app_not_installed", `App is not installed: ${appId}`, { appId });
    }
    const snapshot = this.createSnapshot({ appId, type: "manual" });
    const clearedStorageKeys = this.get("SELECT COUNT(*) AS count FROM app_storage WHERE app_id = ?", appId)?.count ?? 0;
    const createdAt = nowIso();
    this.transaction(() => {
      this.run("DELETE FROM app_storage WHERE app_id = ?", appId);
      this.run("UPDATE app_versions SET status = 'uninstalled' WHERE app_id = ?", appId);
      this.run(
        "UPDATE apps SET status = 'uninstalled', active_install_id = NULL, active_version = NULL, updated_at = ? WHERE id = ?",
        createdAt,
        appId,
      );
      if (app.active_install_id) {
        this.run(
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, 'uninstall', ?, ?, ?, ?)",
          id("installation"),
          appId,
          app.active_install_id,
          app.active_install_id,
          actor,
          createdAt,
          prettyJson({ snapshotId: snapshot.snapshotId, clearedStorageKeys }),
        );
      }
    });
    return { ok: true, appId, status: "uninstalled", snapshotId: snapshot.snapshotId, clearedStorageKeys };
  }

  logControlCommand({
    controlSessionId,
    runtimeSessionId = null,
    tool,
    args = null,
    result = null,
    error = null,
    durationMs = 0,
    httpMethod = null,
    path = null,
    decision = null,
    errorCode = null,
  }) {
    this.run(
      "INSERT INTO control_commands (command_id, control_session_id, runtime_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      id("command"),
      controlSessionId,
      runtimeSessionId,
      tool,
      httpMethod,
      path,
      decision,
      errorCode,
      args ? prettyJson(args) : null,
      result ? prettyJson(result) : null,
      error ? prettyJson(error) : null,
      nowIso(),
      durationMs,
    );
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
    this.recordResourceHighWater({ sessionId, appId });
  }

  logCoreStep({ sessionId, appId, installId = null, event, result }) {
    const createdAt = nowIso();
    const eventId = id("core_event");
    const stateVersion = Number.isInteger(result?.stateVersion) ? result.stateVersion : null;
    this.transaction(() => {
      this.run(
        "INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        eventId,
        sessionId,
        appId,
        installId,
        stateVersion === null ? null : Math.max(0, stateVersion - 1),
        prettyJson(event ?? null),
        createdAt,
      );
      for (const action of result?.actions ?? []) {
        this.run(
          "INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
          id("core_action"),
          eventId,
          sessionId,
          appId,
          prettyJson(action),
          createdAt,
        );
      }
    });
    return { eventId, actionCount: result?.actions?.length ?? 0 };
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

  storageBytesAfterSet(appId, key, value) {
    const valueJson = prettyJson(value);
    const current = this.get(
      "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) AS bytes FROM app_storage WHERE app_id = ?",
      appId,
    )?.bytes ?? 0;
    const existing = this.get(
      "SELECT LENGTH(CAST(value_json AS BLOB)) AS bytes FROM app_storage WHERE app_id = ? AND key = ?",
      appId,
      key,
    )?.bytes ?? 0;
    return current - existing + Buffer.byteLength(valueJson);
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

  resetNetworkMocks({ sessionId = null, appId = null } = {}) {
    if (sessionId && appId) {
      return { ok: true, cleared: this.run("DELETE FROM network_mocks WHERE session_id = ? AND app_id = ?", sessionId, appId).changes };
    }
    if (sessionId) {
      return { ok: true, cleared: this.run("DELETE FROM network_mocks WHERE session_id = ?", sessionId).changes };
    }
    if (appId) {
      return { ok: true, cleared: this.run("DELETE FROM network_mocks WHERE app_id = ?", appId).changes };
    }
    return { ok: true, cleared: this.run("DELETE FROM network_mocks").changes };
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
      control_sessions: this.all("SELECT * FROM control_sessions ORDER BY started_at"),
      control_commands: this.queryControlCommands(),
      runtime_sessions: this.all("SELECT * FROM runtime_sessions ORDER BY started_at"),
      runtime_snapshots: this.all("SELECT * FROM runtime_snapshots ORDER BY created_at"),
      app_migrations: this.all("SELECT * FROM app_migrations ORDER BY created_at"),
      migration_runs: this.all("SELECT * FROM migration_runs ORDER BY started_at"),
      test_runs: this.all("SELECT * FROM test_runs ORDER BY started_at"),
    };
  }

  exportBackup({ type = "backup", runtimeCapabilities = {}, includeDebug = false } = {}) {
    const createdAt = nowIso();
    const document = {
      exportId: id("export"),
      type,
      createdAt,
      runtimeVersion: "0.4.0",
      source: { platform: "fake-host", target: "fake-host" },
      apps: this.all("SELECT * FROM apps ORDER BY id"),
      appVersions: this.all("SELECT * FROM app_versions ORDER BY app_id, created_at"),
      appFiles: this.all("SELECT * FROM app_files ORDER BY install_id, path"),
      appPermissions: this.all("SELECT * FROM app_permissions ORDER BY install_id, permission"),
      appStorage: this.all("SELECT * FROM app_storage ORDER BY app_id, key"),
      appMigrations: this.all("SELECT * FROM app_migrations ORDER BY app_id, from_data_version"),
      appInstallReports: this.all("SELECT * FROM app_install_reports ORDER BY app_id, created_at"),
      runtimeCapabilities,
      debug: includeDebug
        ? {
            runtimeSessions: this.all("SELECT * FROM runtime_sessions ORDER BY started_at"),
            bridgeCalls: this.all("SELECT * FROM bridge_calls ORDER BY created_at"),
            controlSessions: this.all("SELECT * FROM control_sessions ORDER BY started_at"),
            controlCommands: this.queryControlCommands(),
            coreEvents: this.all("SELECT * FROM core_events ORDER BY created_at"),
            coreActions: this.all("SELECT * FROM core_actions ORDER BY created_at"),
            runtimeSnapshots: this.all("SELECT * FROM runtime_snapshots ORDER BY created_at"),
            testRuns: this.all("SELECT * FROM test_runs ORDER BY started_at"),
          }
        : {},
    };
    document.contentHash = `sha256:${sha256(canonicalJson(document))}`;
    this.run(
      "INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at) VALUES (?, ?, 'fake-host', ?, ?, ?, ?)",
      document.exportId,
      type,
      document.runtimeVersion,
      prettyJson(document),
      document.contentHash,
      createdAt,
    );
    return document;
  }

  importBackup(document) {
    if (!document || typeof document !== "object") {
      throw new Error("Backup document must be an object");
    }
    const createdAt = nowIso();
    this.transaction(() => {
      for (const app of document.apps ?? []) {
        this.run(
          "INSERT OR REPLACE INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
          app.id,
          app.name,
          app.status ?? "enabled",
          app.active_install_id ?? app.activeInstallId ?? null,
          app.active_version ?? app.activeVersion ?? null,
          app.data_version ?? app.dataVersion ?? 1,
          app.created_at ?? app.createdAt ?? createdAt,
          app.updated_at ?? app.updatedAt ?? createdAt,
        );
      }

      for (const version of document.appVersions ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
          version.install_id ?? version.installId,
          version.app_id ?? version.appId,
          version.version ?? version.appVersion,
          version.runtime_version ?? version.runtimeVersion ?? "0.1.0",
          version.data_version ?? version.dataVersion ?? 1,
          version.manifest_json ?? version.manifestJson ?? prettyJson(version.manifest ?? {}),
          version.manifest_hash ?? version.manifestHash ?? "",
          version.content_hash ?? version.contentHash ?? "",
          version.signature_json ?? version.signatureJson ?? (version.signature ? prettyJson(version.signature) : null),
          version.trust_level ?? version.trustLevel ?? "developer",
          version.status ?? "installed",
          version.created_at ?? version.installedAt ?? createdAt,
          version.activated_at ?? version.activatedAt ?? null,
        );
      }

      for (const file of document.appFiles ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
          file.install_id ?? file.installId,
          file.path,
          file.content_text ?? file.contentText ?? "",
          file.content_hash ?? file.contentHash ?? "",
          file.size_bytes ?? file.sizeBytes ?? Buffer.byteLength(file.content_text ?? file.contentText ?? ""),
          file.mime ?? "text/plain",
          file.created_at ?? file.createdAt ?? createdAt,
        );
      }

      for (const permission of document.appPermissions ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) VALUES (?, ?, ?, ?, ?, ?, ?)",
          permission.install_id ?? permission.installId,
          permission.app_id ?? permission.appId,
          permission.permission,
          permission.requested ?? 1,
          permission.approved === true ? 1 : permission.approved ?? 0,
          permission.approved_at ?? permission.approvedAt ?? null,
          permission.reason ?? "imported",
        );
      }

      for (const storage of document.appStorage ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
          storage.app_id ?? storage.appId,
          storage.key,
          storage.value_json ?? storage.valueJson ?? prettyJson(storage.value ?? null),
          storage.updated_at ?? storage.updatedAt ?? createdAt,
        );
      }

      for (const migration of document.appMigrations ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
          migration.migration_id ?? migration.migrationId,
          migration.app_id ?? migration.appId,
          migration.from_data_version ?? migration.fromDataVersion,
          migration.to_data_version ?? migration.toDataVersion,
          migration.migration_json ?? migration.migrationJson ?? prettyJson(migration.migration ?? {}),
          migration.content_hash ?? migration.contentHash ?? "",
          migration.created_at ?? migration.createdAt ?? createdAt,
        );
      }

      for (const report of document.appInstallReports ?? []) {
        this.run(
          "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
          report.report_id ?? report.reportId,
          report.app_id ?? report.appId,
          report.install_id ?? report.installId,
          report.status ?? "accepted",
          report.validation_json ?? report.validationJson ?? null,
          report.security_json ?? report.securityJson ?? null,
          report.permissions_json ?? report.permissionsJson ?? null,
          report.compatibility_json ?? report.compatibilityJson ?? null,
          report.smoke_test_json ?? report.smokeTestJson ?? null,
          report.content_hash ?? report.contentHash ?? null,
          report.created_at ?? report.createdAt ?? createdAt,
        );
      }

      this.run(
        "INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at) VALUES (?, 'import', ?, ?, ?, ?, ?, ?)",
        id("import"),
        document.source?.platform ?? "unknown",
        document.runtimeVersion ?? "0.4.0",
        prettyJson(document),
        document.contentHash ?? `sha256:${sha256(canonicalJson(document))}`,
        createdAt,
        createdAt,
      );
    });

    return {
      ok: true,
      apps: (document.apps ?? []).length,
      appVersions: (document.appVersions ?? []).length,
      appStorage: (document.appStorage ?? []).length,
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

  queryConsoleLogs(appId = null) {
    const rows = appId
      ? this.all("SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at", appId)
      : this.all("SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at");
    return rows.map((row) => {
      const params = row.params_json ? JSON.parse(row.params_json) : {};
      return {
        bridgeCallId: row.bridge_call_id,
        appId: row.app_id,
        level: params.level ?? null,
        message: params.message ?? null,
        params,
        result: row.result_json ? JSON.parse(row.result_json) : null,
        error: row.error_json ? JSON.parse(row.error_json) : null,
        createdAt: row.created_at,
      };
    });
  }

  queryNotifications(appId = null) {
    const rows = appId
      ? this.all("SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' AND app_id = ? ORDER BY created_at", appId)
      : this.all("SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' ORDER BY created_at");
    return rows.map((row) => {
      const params = row.params_json ? JSON.parse(row.params_json) : {};
      return {
        bridgeCallId: row.bridge_call_id,
        appId: row.app_id,
        message: params.message ?? null,
        level: params.level ?? null,
        params,
        result: row.result_json ? JSON.parse(row.result_json) : null,
        error: row.error_json ? JSON.parse(row.error_json) : null,
        createdAt: row.created_at,
      };
    });
  }

  queryControlCommands(controlSessionId = null) {
    if (controlSessionId) {
      return this.all("SELECT * FROM control_commands WHERE control_session_id = ? ORDER BY created_at", controlSessionId);
    }
    return this.all("SELECT * FROM control_commands ORDER BY created_at");
  }

  countBridgeCallsSince({ appId, since, method = null, installId = null }) {
    if (method && installId) {
      return this.get(
        "SELECT COUNT(*) AS count FROM bridge_calls WHERE app_id = ? AND install_id = ? AND method = ? AND created_at >= ?",
        appId,
        installId,
        method,
        since,
      )?.count ?? 0;
    }
    if (method) {
      return this.get(
        "SELECT COUNT(*) AS count FROM bridge_calls WHERE app_id = ? AND method = ? AND created_at >= ?",
        appId,
        method,
        since,
      )?.count ?? 0;
    }
    if (installId) {
      return this.get(
        "SELECT COUNT(*) AS count FROM bridge_calls WHERE app_id = ? AND install_id = ? AND created_at >= ?",
        appId,
        installId,
        since,
      )?.count ?? 0;
    }
    return this.get(
      "SELECT COUNT(*) AS count FROM bridge_calls WHERE app_id = ? AND created_at >= ?",
      appId,
      since,
    )?.count ?? 0;
  }

  countBridgeErrorsSince({ appId, since, code, installId = null }) {
    const rows = installId
      ? this.all(
        "SELECT error_json FROM bridge_calls WHERE app_id = ? AND install_id = ? AND error_json IS NOT NULL AND created_at >= ?",
        appId,
        installId,
        since,
      )
      : this.all(
        "SELECT error_json FROM bridge_calls WHERE app_id = ? AND error_json IS NOT NULL AND created_at >= ?",
        appId,
        since,
      );
    return rows.filter((row) => {
      try {
        return JSON.parse(row.error_json)?.code === code;
      } catch {
        return false;
      }
    }).length;
  }

  queryCoreEvents(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM core_events WHERE app_id = ? ORDER BY created_at", appId);
    }
    return this.all("SELECT * FROM core_events ORDER BY created_at");
  }

  queryCoreActions(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM core_actions WHERE app_id = ? ORDER BY created_at", appId);
    }
    return this.all("SELECT * FROM core_actions ORDER BY created_at");
  }

  queryTestRuns(appId = null) {
    if (appId) {
      return this.all("SELECT * FROM test_runs WHERE app_id = ? ORDER BY started_at", appId);
    }
    return this.all("SELECT * FROM test_runs ORDER BY started_at");
  }

  recordTestRun({ microTestId, name, appId, spec, status, result }) {
    const startedAt = nowIso();
    const testRunId = id("testrun");
    this.run(
      "INSERT INTO micro_tests (micro_test_id, app_id, name, spec_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(micro_test_id) DO UPDATE SET app_id = excluded.app_id, name = excluded.name, spec_json = excluded.spec_json, updated_at = excluded.updated_at",
      microTestId,
      appId,
      name,
      prettyJson(spec),
      startedAt,
      startedAt,
    );
    this.run(
      "INSERT INTO test_runs (test_run_id, micro_test_id, app_id, status, started_at, finished_at, result_json, diagnostics_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      testRunId,
      microTestId,
      appId,
      status,
      startedAt,
      nowIso(),
      prettyJson(result),
      prettyJson({ runner: "fake-host-static" }),
    );
    return { testRunId, microTestId, appId, status, result };
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

function emptyResourceHighWater(appId) {
  return {
    appId,
    storageBytes: 0,
    bridgeCallsLastMinute: 0,
    networkRequestsLastMinute: 0,
    logLinesLastMinute: 0,
    updatedAt: null,
  };
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
