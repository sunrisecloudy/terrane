import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "node:path";
import { sqliteMigrationsDir } from "./paths.js";
import { id, nowIso, prettyJson } from "./util.js";

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

  insertInstalledPackage({ manifest, files, hashes, validation, trustLevel = "local-dev" }) {
    const createdAt = nowIso();
    const installId = `install_${manifest.id}_${manifest.version}_${hashes.contentHash.slice(0, 12)}`;
    const reportId = id("report");

    this.transaction(() => {
      this.run(
        "INSERT INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = 'enabled', data_version = excluded.data_version, updated_at = excluded.updated_at",
        manifest.id,
        manifest.name,
        manifest.dataVersion,
        createdAt,
        createdAt,
      );

      this.run(
        "INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'enabled', ?, ?)",
        installId,
        manifest.id,
        manifest.version,
        manifest.runtimeVersion,
        manifest.dataVersion,
        prettyJson(manifest),
        hashes.manifestHash,
        hashes.contentHash,
        prettyJson(devSignature(manifest, hashes)),
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
        prettyJson({ ok: true, mode: "dev-none-signature" }),
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
        prettyJson({ previousInstallId: this.activeInstallId(manifest.id) }),
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

  activeInstall(appId) {
    const row = this.get(
      "SELECT apps.id AS app_id, apps.active_install_id, apps.active_version, app_versions.manifest_json FROM apps LEFT JOIN app_versions ON app_versions.install_id = apps.active_install_id WHERE apps.id = ?",
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
      test_runs: this.all("SELECT * FROM test_runs ORDER BY started_at"),
    };
  }

  queryAppStorage(appId) {
    return this.all("SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key", appId);
  }

  queryAppVersions(appId) {
    return this.all(
      "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, trust_level, status, created_at, activated_at FROM app_versions WHERE app_id = ? ORDER BY created_at",
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

function devSignature(manifest, hashes) {
  return {
    algorithm: "none-dev",
    appId: manifest.id,
    version: manifest.version,
    manifestHash: hashes.manifestHash,
    contentHash: hashes.contentHash,
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
