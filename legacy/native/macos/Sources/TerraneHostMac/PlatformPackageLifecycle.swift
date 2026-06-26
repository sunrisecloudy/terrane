import Foundation
import SQLite3

enum PlatformPackageLifecycle {
    private static let core = ForgeCoreBridge()

    static var isAvailable: Bool { core.isAvailable }

    static func syncRegistry(database: OpaquePointer?) throws {
        guard let database, isAvailable else { return }
        let snapshot = try exportSnapshot(database: database)
        _ = try core.command(name: "package.provision_registry", payload: ["snapshot": snapshot])
    }

    static func rollback(
        database: OpaquePointer?,
        appId: String,
        targetInstallId: String? = nil,
        actor: String,
        createdAt: String,
        eventId: String
    ) throws -> [String: Any] {
        guard isAvailable else {
            throw PlatformAppRegistryError.sqlite("package.rollback_version requires forge core")
        }
        try syncRegistry(database: database)
        var payload: [String: Any] = [
            "app_id": appId,
            "created_at": createdAt,
            "installation_event_id": eventId,
            "actor": actor,
        ]
        if let targetInstallId {
            payload["target_install_id"] = targetInstallId
        }
        guard let transition = try core.command(name: "package.rollback_version", payload: payload) as? [String: Any] else {
            throw PlatformAppRegistryError.sqlite("package.rollback_version returned invalid payload")
        }
        try applySqlOps(database: database, transition: transition)
        return transition
    }

    static func setStatus(
        database: OpaquePointer?,
        appId: String,
        installId: String,
        status: String,
        actor: String,
        createdAt: String,
        reason: String?,
        restorePrevious: Bool,
        eventId: String
    ) throws -> [String: Any] {
        guard isAvailable else {
            throw PlatformAppRegistryError.sqlite("package.set_status requires forge core")
        }
        try syncRegistry(database: database)
        var payload: [String: Any] = [
            "app_id": appId,
            "install_id": installId,
            "status": status,
            "created_at": createdAt,
            "restore_previous": restorePrevious,
            "installation_event_id": eventId,
            "actor": actor,
        ]
        if let reason {
            payload["reason"] = reason
        }
        guard let transition = try core.command(name: "package.set_status", payload: payload) as? [String: Any] else {
            throw PlatformAppRegistryError.sqlite("package.set_status returned invalid payload")
        }
        try applySqlOps(database: database, transition: transition)
        return transition
    }

    static func autoQuarantine(
        database: OpaquePointer?,
        appId: String,
        installId: String,
        budgetErrorCount60s: Int,
        error: [String: Any],
        actor: String,
        createdAt: String
    ) throws -> [String: Any]? {
        guard isAvailable else { return nil }
        try syncRegistry(database: database)
        let payload: [String: Any] = [
            "app_id": appId,
            "install_id": installId,
            "budget_error_count_60s": budgetErrorCount60s,
            "is_active_install": true,
            "error": error,
            "created_at": createdAt,
            "actor": actor,
        ]
        guard let decision = try core.command(name: "quota.auto_quarantine", payload: payload) as? [String: Any] else {
            return nil
        }
        guard decision["should_quarantine"] as? Bool == true,
              let transition = decision["transition"] as? [String: Any]
        else {
            return decision
        }
        try applySqlOps(database: database, transition: transition)
        return decision
    }

    private static func applySqlOps(database: OpaquePointer?, transition: [String: Any]) throws {
        guard let database,
              let ops = transition["sql_ops"] as? [[String: Any]]
        else {
            return
        }
        guard sqlite3_exec(database, "BEGIN IMMEDIATE", nil, nil, nil) == SQLITE_OK else {
            throw PlatformAppRegistryError.sqlite("begin transaction failed")
        }
        var ok = true
        for op in ops {
            guard let sql = op["sql"] as? String else { continue }
            let args = op["args"] as? [Any] ?? []
            ok = ok && executePrepared(database: database, sql: sql, values: args)
        }
        if ok, sqlite3_exec(database, "COMMIT", nil, nil, nil) == SQLITE_OK {
            return
        }
        _ = sqlite3_exec(database, "ROLLBACK", nil, nil, nil)
        throw PlatformAppRegistryError.sqlite("apply package sql_ops failed")
    }

    private static func exportSnapshot(database: OpaquePointer) throws -> [String: Any] {
        var apps: [String: Any] = [:]
        var versions: [String: Any] = [:]
        for row in queryRows(
            database: database,
            sql: "SELECT id, name, status, active_install_id, active_version, data_version FROM apps"
        ) {
            guard let id = row["id"] as? String else { continue }
            apps[id] = row
        }
        for row in queryRows(
            database: database,
            sql: "SELECT install_id, app_id, version, runtime_version, data_version, status, created_at, activated_at FROM app_versions"
        ) {
            guard let installId = row["install_id"] as? String else { continue }
            versions[installId] = row
        }
        return [
            "apps": apps,
            "versions": versions,
            "next_event_seq": 0,
        ]
    }

    private static func queryRows(database: OpaquePointer, sql: String) -> [[String: Any]] {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            var row: [String: Any] = [:]
            let count = sqlite3_column_count(statement)
            for index in 0..<count {
                guard let name = sqlite3_column_name(statement, index) else { continue }
                let key = String(cString: name)
                if sqlite3_column_type(statement, index) == SQLITE_NULL {
                    row[key] = NSNull()
                } else if let text = sqlite3_column_text(statement, index) {
                    row[key] = String(cString: text)
                } else {
                    row[key] = Int(sqlite3_column_int64(statement, index))
                }
            }
            rows.append(row)
        }
        return rows
    }

    private static func executePrepared(database: OpaquePointer, sql: String, values: [Any]) -> Bool {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            let position = Int32(index + 1)
            if value is NSNull {
                sqlite3_bind_null(statement, position)
            } else if let string = value as? String {
                sqlite3_bind_text(statement, position, string, -1, SQLITE_TRANSIENT_PACKAGE_LIFECYCLE)
            } else if let number = value as? Int {
                sqlite3_bind_int64(statement, position, Int64(number))
            } else if let number = value as? Int64 {
                sqlite3_bind_int64(statement, position, number)
            } else if let number = value as? NSNumber {
                if CFGetTypeID(number) == CFBooleanGetTypeID() {
                    sqlite3_bind_int64(statement, position, number.boolValue ? 1 : 0)
                } else {
                    sqlite3_bind_int64(statement, position, number.int64Value)
                }
            } else if JSONSerialization.isValidJSONObject(value),
                      let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
                      let string = String(data: data, encoding: .utf8) {
                sqlite3_bind_text(statement, position, string, -1, SQLITE_TRANSIENT_PACKAGE_LIFECYCLE)
            } else {
                sqlite3_bind_null(statement, position)
            }
        }
        return sqlite3_step(statement) == SQLITE_DONE
    }
}

private let SQLITE_TRANSIENT_PACKAGE_LIFECYCLE = unsafeBitCast(-1, to: sqlite3_destructor_type.self)