import Foundation
import SQLite3

final class PlatformDatabase {
    private(set) var handle: OpaquePointer?

    init(databaseURL: URL? = nil) {
        let url = databaseURL ?? Self.defaultDatabaseURL()
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard sqlite3_open(url.path, &handle) == SQLITE_OK else {
            return
        }

        execute("PRAGMA foreign_keys = ON;")
        applyMigrations()
        runIntegrityCheck()
    }

    deinit {
        sqlite3_close(handle)
    }

    private static func defaultDatabaseURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("Terrane/platform.sqlite")
    }

    private func applyMigrations() {
        guard let migrationsURL = RuntimeResourceLocator.sqliteMigrationsDirectoryURL() else {
            executeFallbackSchema()
            return
        }

        guard let migrations = try? FileManager.default.contentsOfDirectory(
            at: migrationsURL,
            includingPropertiesForKeys: nil
        )
            .filter({ $0.pathExtension == "sql" })
            .sorted(by: { $0.lastPathComponent < $1.lastPathComponent })
        else {
            executeFallbackSchema()
            return
        }

        for migration in migrations {
            if let sql = try? String(contentsOf: migration, encoding: .utf8) {
                execute(sql)
            }
        }
    }

    private func executeFallbackSchema() {
        execute(
            """
            CREATE TABLE IF NOT EXISTS apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'enabled', data_version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));
            """
        )
    }

    private func runIntegrityCheck() {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(handle, "PRAGMA integrity_check", -1, &statement, nil) == SQLITE_OK else {
            return
        }
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW,
              let text = sqlite3_column_text(statement, 0),
              String(cString: text) != "ok"
        else {
            return
        }
        fputs("PlatformDatabase integrity_check failed: \(String(cString: text))\n", stderr)
    }

    private func execute(_ sql: String) {
        var error: UnsafeMutablePointer<CChar>?
        if sqlite3_exec(handle, sql, nil, nil, &error) != SQLITE_OK {
            let message = error.map { String(cString: $0) } ?? "sqlite error"
            fputs("PlatformDatabase failed to apply SQL: \(message)\n", stderr)
        }
        sqlite3_free(error)
    }
}
