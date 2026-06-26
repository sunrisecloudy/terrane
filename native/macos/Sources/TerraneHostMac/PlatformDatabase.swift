import Foundation
import SQLite3

enum PlatformDatabaseError: Error, Equatable {
    case openFailed(String)
    case migrationsUnavailable
    case migrationFailed(String)
}

final class PlatformDatabase {
    private(set) var handle: OpaquePointer?

    init(databaseURL: URL? = nil) {
        do {
            try open(databaseURL: databaseURL)
        } catch {
            fputs("PlatformDatabase failed to initialize: \(error)\n", stderr)
        }
    }

    private func open(databaseURL: URL?) throws {
        let url = databaseURL ?? Self.defaultDatabaseURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        var opened: OpaquePointer?
        guard sqlite3_open(url.path, &opened) == SQLITE_OK, let opened else {
            throw PlatformDatabaseError.openFailed("sqlite open failed for \(url.path)")
        }
        handle = opened

        try executeOrThrow("PRAGMA foreign_keys = ON;")
        try applyMigrations()
        runIntegrityCheck()
    }

    deinit {
        sqlite3_close(handle)
    }

    private static func defaultDatabaseURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("Terrane/platform.sqlite")
    }

    private func applyMigrations() throws {
        guard let migrationsURL = RuntimeResourceLocator.sqliteMigrationsDirectoryURL() else {
            throw PlatformDatabaseError.migrationsUnavailable
        }

        let migrations = try FileManager.default.contentsOfDirectory(
            at: migrationsURL,
            includingPropertiesForKeys: nil
        )
            .filter({ $0.pathExtension == "sql" })
            .sorted(by: { $0.lastPathComponent < $1.lastPathComponent })
        guard !migrations.isEmpty else {
            throw PlatformDatabaseError.migrationsUnavailable
        }

        for migration in migrations {
            let sql = try String(contentsOf: migration, encoding: .utf8)
            try executeOrThrow(sql)
        }
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

    private func executeOrThrow(_ sql: String) throws {
        guard let handle else {
            throw PlatformDatabaseError.openFailed("database handle is not available")
        }
        var error: UnsafeMutablePointer<CChar>?
        if sqlite3_exec(handle, sql, nil, nil, &error) != SQLITE_OK {
            let message = error.map { String(cString: $0) } ?? "sqlite error"
            sqlite3_free(error)
            throw PlatformDatabaseError.migrationFailed(message)
        }
        sqlite3_free(error)
    }
}
