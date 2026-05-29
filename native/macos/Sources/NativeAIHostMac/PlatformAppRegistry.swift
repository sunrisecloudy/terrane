import Foundation
import SQLite3

private let SQLITE_TRANSIENT_REGISTRY = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

struct InstalledAppVersion: Equatable {
    let installId: String
    let appId: String
    let version: String
    let dataVersion: Int
    let status: String
}

struct AppInstallationEvent: Equatable {
    let action: String
    let installId: String
    let previousInstallId: String?
    let actor: String
}

struct AppRollbackResult: Equatable {
    let appId: String
    let activeInstallId: String
    let rolledBackInstallId: String
    let activeVersion: String
}

enum PlatformAppRegistryError: Error, Equatable {
    case appNotInstalled
    case noRollbackTarget
    case rollbackDataVersionIncompatible
    case sqlite(String)
}

final class PlatformAppRegistry {
    typealias RollbackSmokeTest = (InstalledAppVersion) throws -> Void

    private var db: OpaquePointer?
    private let configuredDatabaseURL: URL?
    private let rollbackSmokeTest: RollbackSmokeTest

    init(databaseURL: URL? = nil, rollbackSmokeTest: @escaping RollbackSmokeTest = { _ in }) throws {
        self.configuredDatabaseURL = databaseURL
        self.rollbackSmokeTest = rollbackSmokeTest
        let url = resolvedDatabaseURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard sqlite3_open(url.path, &db) == SQLITE_OK else {
            throw PlatformAppRegistryError.sqlite("sqlite open failed")
        }
        try execute("PRAGMA foreign_keys = ON;")
        try ensureSchema()
    }

    deinit {
        sqlite3_close(db)
    }

    @discardableResult
    func installVersion(
        appId: String,
        name: String,
        version: String,
        runtimeVersion: String = "0.1.0",
        dataVersion: Int = 1,
        manifestJSON: String,
        contentHash: String,
        installId: String,
        activate: Bool = true,
        actor: String = "macos-test"
    ) throws -> InstalledAppVersion {
        let createdAt = now()
        try transaction {
            try execute(
                """
                INSERT INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at)
                VALUES (?, ?, 'enabled', NULL, NULL, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = 'enabled', updated_at = excluded.updated_at
                """,
                [appId, name, String(dataVersion), createdAt, createdAt]
            )

            if activate, let current = try activeVersion(appId: appId) {
                try execute("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", [current.installId])
            }

            let status = activate ? "enabled" : "installed"
            let activatedAt = activate ? createdAt : nil
            try execute(
                """
                INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, 'developer', ?, ?, ?)
                """,
                [
                    installId,
                    appId,
                    version,
                    runtimeVersion,
                    String(dataVersion),
                    manifestJSON,
                    "sha256:\(contentHash)",
                    contentHash,
                    status,
                    createdAt,
                    activatedAt
                ]
            )

            try insertInstallationEvent(
                appId: appId,
                installId: installId,
                action: "install",
                previousInstallId: nil,
                actor: actor,
                createdAt: createdAt
            )

            if activate {
                try execute(
                    "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, updated_at = ? WHERE id = ?",
                    [installId, version, String(dataVersion), createdAt, appId]
                )
                try insertInstallationEvent(
                    appId: appId,
                    installId: installId,
                    action: "activate",
                    previousInstallId: nil,
                    actor: actor,
                    createdAt: createdAt
                )
            }
        }

        return try requireVersion(installId: installId)
    }

    func rollback(appId: String, actor: String = "macos-test") throws -> AppRollbackResult {
        let createdAt = now()
        var result: AppRollbackResult?
        try transaction {
            guard let current = try activeVersion(appId: appId) else {
                throw PlatformAppRegistryError.appNotInstalled
            }
            guard let target = try rollbackTarget(appId: appId, excluding: current.installId) else {
                throw PlatformAppRegistryError.noRollbackTarget
            }
            guard target.dataVersion == current.dataVersion else {
                throw PlatformAppRegistryError.rollbackDataVersionIncompatible
            }

            try execute("UPDATE app_versions SET status = 'rolled-back' WHERE install_id = ?", [current.installId])
            try execute("UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", [createdAt, target.installId])
            try execute(
                "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, updated_at = ? WHERE id = ?",
                [target.installId, target.version, String(target.dataVersion), createdAt, appId]
            )
            try rollbackSmokeTest(target)
            try insertInstallationEvent(
                appId: appId,
                installId: target.installId,
                action: "rollback",
                previousInstallId: current.installId,
                actor: actor,
                createdAt: createdAt
            )

            result = AppRollbackResult(
                appId: appId,
                activeInstallId: target.installId,
                rolledBackInstallId: current.installId,
                activeVersion: target.version
            )
        }
        return try require(result)
    }

    func activeVersion(appId: String) throws -> InstalledAppVersion? {
        try queryVersion(
            """
            SELECT v.install_id, v.app_id, v.version, v.data_version, v.status
            FROM apps a JOIN app_versions v ON v.install_id = a.active_install_id
            WHERE a.id = ? AND a.status = 'enabled'
            """,
            [appId]
        )
    }

    func installationEvents(appId: String) throws -> [AppInstallationEvent] {
        var statement: OpaquePointer?
        let sql = """
        SELECT action, install_id, previous_install_id, actor
        FROM app_installations
        WHERE app_id = ?
        ORDER BY created_at, installation_event_id
        """
        try prepare(sql, &statement)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)

        var events: [AppInstallationEvent] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            events.append(AppInstallationEvent(
                action: columnText(statement, 0) ?? "",
                installId: columnText(statement, 1) ?? "",
                previousInstallId: columnText(statement, 2),
                actor: columnText(statement, 3) ?? ""
            ))
        }
        return events
    }

    private func rollbackTarget(appId: String, excluding installId: String) throws -> InstalledAppVersion? {
        try queryVersion(
            """
            SELECT install_id, app_id, version, data_version, status
            FROM app_versions
            WHERE app_id = ? AND install_id != ? AND status IN ('installed','enabled')
            ORDER BY created_at DESC
            LIMIT 1
            """,
            [appId, installId]
        )
    }

    private func requireVersion(installId: String) throws -> InstalledAppVersion {
        let version = try queryVersion(
            "SELECT install_id, app_id, version, data_version, status FROM app_versions WHERE install_id = ?",
            [installId]
        )
        guard let version else {
            throw PlatformAppRegistryError.sqlite("installed version not found")
        }
        return version
    }

    private func queryVersion(_ sql: String, _ values: [String]) throws -> InstalledAppVersion? {
        var statement: OpaquePointer?
        try prepare(sql, &statement)
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bind(statement, Int32(index + 1), value)
        }

        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return InstalledAppVersion(
            installId: columnText(statement, 0) ?? "",
            appId: columnText(statement, 1) ?? "",
            version: columnText(statement, 2) ?? "",
            dataVersion: Int(sqlite3_column_int64(statement, 3)),
            status: columnText(statement, 4) ?? ""
        )
    }

    private func insertInstallationEvent(
        appId: String,
        installId: String,
        action: String,
        previousInstallId: String?,
        actor: String,
        createdAt: String
    ) throws {
        try execute(
            """
            INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json)
            VALUES (?, ?, ?, ?, ?, ?, NULL, ?, '{}')
            """,
            ["event-\(UUID().uuidString)", appId, installId, action, previousInstallId, actor, createdAt]
        )
    }

    private func ensureSchema() throws {
        try execute(
            """
            CREATE TABLE IF NOT EXISTS apps (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              status TEXT NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled','quarantined','uninstalled')),
              active_install_id TEXT,
              active_version TEXT,
              data_version INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_versions (
              install_id TEXT PRIMARY KEY,
              app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
              version TEXT NOT NULL,
              runtime_version TEXT NOT NULL,
              data_version INTEGER NOT NULL,
              manifest_json TEXT NOT NULL,
              manifest_hash TEXT NOT NULL,
              content_hash TEXT NOT NULL,
              signature_json TEXT,
              trust_level TEXT NOT NULL DEFAULT 'user-generated',
              status TEXT NOT NULL DEFAULT 'installed' CHECK (status IN ('installed','enabled','disabled','quarantined','rolled-back','uninstalled')),
              created_at TEXT NOT NULL,
              activated_at TEXT
            );
            CREATE TABLE IF NOT EXISTS app_installations (
              installation_event_id TEXT PRIMARY KEY,
              app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
              install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
              action TEXT NOT NULL CHECK (action IN ('install','activate','disable','rollback','quarantine','uninstall','import')),
              previous_install_id TEXT,
              actor TEXT NOT NULL DEFAULT 'system',
              report_id TEXT,
              created_at TEXT NOT NULL,
              details_json TEXT
            );
            CREATE TABLE IF NOT EXISTS app_storage (
              app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
              key TEXT NOT NULL,
              value_json TEXT,
              updated_at TEXT NOT NULL,
              PRIMARY KEY (app_id, key)
            );
            CREATE INDEX IF NOT EXISTS idx_app_versions_app_version ON app_versions(app_id, version);
            CREATE INDEX IF NOT EXISTS idx_app_installations_app_created ON app_installations(app_id, created_at);
            """
        )
    }

    private func transaction(_ body: () throws -> Void) throws {
        try execute("BEGIN IMMEDIATE TRANSACTION;")
        do {
            try body()
            try execute("COMMIT;")
        } catch {
            try? execute("ROLLBACK;")
            throw error
        }
    }

    private func execute(_ sql: String, _ values: [String?] = []) throws {
        if values.isEmpty {
            guard sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK else {
                throw sqliteError()
            }
            return
        }

        var statement: OpaquePointer?
        try prepare(sql, &statement)
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            if let value {
                bind(statement, Int32(index + 1), value)
            } else {
                sqlite3_bind_null(statement, Int32(index + 1))
            }
        }
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw sqliteError()
        }
    }

    private func prepare(_ sql: String, _ statement: inout OpaquePointer?) throws {
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw sqliteError()
        }
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_REGISTRY)
    }

    private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
        guard let value = sqlite3_column_text(statement, index) else {
            return nil
        }
        return String(cString: value)
    }

    private func sqliteError() -> PlatformAppRegistryError {
        if let message = sqlite3_errmsg(db) {
            return .sqlite(String(cString: message))
        }
        return .sqlite("sqlite error")
    }

    private func resolvedDatabaseURL() -> URL {
        if let configuredDatabaseURL {
            return configuredDatabaseURL
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("NativeAIWebappPlatform/platform.sqlite")
    }

    private func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    private func require<T>(_ value: T?) throws -> T {
        guard let value else {
            throw PlatformAppRegistryError.sqlite("missing rollback result")
        }
        return value
    }
}
