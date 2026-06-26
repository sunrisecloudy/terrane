import Foundation
import SQLite3

enum BridgeBudgetQuarantine {
    static func activeInstallId(database: OpaquePointer?, appId: String) -> String? {
        activeAppRecord(database: database, appId: appId)?.activeInstallId
    }

    static func maybeQuarantineAfterBudgetError(
        database: OpaquePointer?,
        appId: String,
        installId: String?,
        error: [String: Any]?,
        actor: String
    ) {
        guard let database,
              error?["code"] as? String == "resource_budget_exceeded",
              let installId,
              activeInstallId(database: database, appId: appId) == installId
        else {
            return
        }
        let count = bridgeBudgetErrorCountSince(database: database, appId: appId, installId: installId, seconds: 60)
        if PlatformPackageLifecycle.isAvailable {
            _ = try? PlatformPackageLifecycle.autoQuarantine(
                database: database,
                appId: appId,
                installId: installId,
                budgetErrorCount60s: count,
                error: error ?? [:],
                actor: actor,
                createdAt: now()
            )
            return
        }
        guard count >= 3 else { return }
        quarantineWebapp(
            database: database,
            appId: appId,
            installId: installId,
            reason: "resource_budget_exceeded",
            restorePrevious: true,
            actor: actor
        )
    }

    private static func bridgeBudgetErrorCountSince(
        database: OpaquePointer,
        appId: String,
        installId: String,
        seconds: Int
    ) -> Int {
        scalarInt(
            database: database,
            sql: """
            SELECT COUNT(*)
            FROM bridge_calls
            WHERE app_id = ?
              AND install_id = ?
              AND error_json LIKE ?
              AND datetime(created_at) >= datetime('now', ?)
            """,
            values: [appId, installId, #"%"code":"resource_budget_exceeded"%"#, "-\(seconds) seconds"]
        )
    }

    private static func activeAppRecord(database: OpaquePointer?, appId: String) -> (status: String, activeInstallId: String?)? {
        guard let database else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT status, active_install_id FROM apps WHERE id = ?"
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            status: columnText(statement, 0),
            activeInstallId: columnNullableText(statement, 1)
        )
    }

    private static func versionRecord(
        database: OpaquePointer,
        appId: String,
        installId: String
    ) -> (installId: String, version: String, dataVersion: Int, status: String)? {
        var statement: OpaquePointer?
        let sql = "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id = ?"
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            installId: columnText(statement, 0),
            version: columnText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2)),
            status: columnText(statement, 3)
        )
    }

    private static func previousRestorableVersion(
        database: OpaquePointer,
        appId: String,
        excluding installId: String
    ) -> (installId: String, version: String, dataVersion: Int)? {
        var statement: OpaquePointer?
        let sql = """
        SELECT install_id, version, data_version
        FROM app_versions
        WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled')
        ORDER BY created_at DESC
        LIMIT 1
        """
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            installId: columnText(statement, 0),
            version: columnText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2))
        )
    }

    private static func quarantineWebapp(
        database: OpaquePointer,
        appId: String,
        installId: String,
        reason: String,
        restorePrevious: Bool,
        actor: String
    ) {
        guard let app = activeAppRecord(database: database, appId: appId),
              app.status != "uninstalled",
              let target = versionRecord(database: database, appId: appId, installId: installId),
              target.status != "uninstalled"
        else {
            return
        }
        let restoreTarget = restorePrevious && app.activeInstallId == installId
            ? previousRestorableVersion(database: database, appId: appId, excluding: installId)
            : nil
        let createdAt = now()

        guard executeSQL(database: database, sql: "BEGIN IMMEDIATE") else { return }
        var ok = true
        ok = ok && executePrepared(
            database: database,
            sql: "UPDATE app_versions SET status = 'quarantined' WHERE app_id = ? AND install_id = ?",
            values: [appId, installId]
        )
        if let restoreTarget {
            ok = ok && executePrepared(
                database: database,
                sql: "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?",
                values: [createdAt, restoreTarget.installId]
            )
            ok = ok && executePrepared(
                database: database,
                sql: "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
                values: [restoreTarget.installId, restoreTarget.version, restoreTarget.dataVersion, createdAt, appId]
            )
            ok = ok && executePrepared(
                database: database,
                sql: """
                INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json)
                VALUES (?, ?, ?, 'rollback', ?, ?, ?, ?)
                """,
                values: [
                    "event_\(UUID().uuidString.lowercased())",
                    appId,
                    restoreTarget.installId,
                    installId,
                    actor,
                    createdAt,
                    jsonBody(["reason": "automatic rollback after quarantine", "quarantinedInstallId": installId] as [String: Any]),
                ]
            )
        } else if app.activeInstallId == installId {
            ok = ok && executePrepared(
                database: database,
                sql: "UPDATE apps SET status = 'quarantined', updated_at = ? WHERE id = ?",
                values: [createdAt, appId]
            )
        }
        ok = ok && executePrepared(
            database: database,
            sql: """
            INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json)
            VALUES (?, ?, ?, 'quarantine', ?, ?, ?, ?)
            """,
            values: [
                "event_\(UUID().uuidString.lowercased())",
                appId,
                target.installId,
                restoreTarget?.installId,
                actor,
                createdAt,
                jsonBody(["reason": reason, "restoredInstallId": restoreTarget?.installId ?? NSNull()] as [String: Any]),
            ]
        )

        if ok, executeSQL(database: database, sql: "COMMIT") {
            return
        }
        _ = executeSQL(database: database, sql: "ROLLBACK")
    }

    private static func executeSQL(database: OpaquePointer, sql: String) -> Bool {
        var error: UnsafeMutablePointer<CChar>?
        let status = sqlite3_exec(database, sql, nil, nil, &error)
        sqlite3_free(error)
        return status == SQLITE_OK
    }

    private static func executePrepared(database: OpaquePointer, sql: String, values: [Any?]) -> Bool {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bindAny(statement, Int32(index + 1), value)
        }
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private static func scalarInt(database: OpaquePointer, sql: String, values: [String]) -> Int {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bind(statement, Int32(index + 1), value)
        }
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return 0
        }
        return Int(sqlite3_column_int64(statement, 0))
    }

    private static func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_BUDGET_QUARANTINE)
    }

    private static func bindAny(_ statement: OpaquePointer?, _ index: Int32, _ value: Any?) {
        guard let value, !(value is NSNull) else {
            sqlite3_bind_null(statement, index)
            return
        }
        if let value = value as? String {
            bind(statement, index, value)
        } else if let value = value as? Int {
            sqlite3_bind_int64(statement, index, Int64(value))
        } else if let value = value as? Int64 {
            sqlite3_bind_int64(statement, index, value)
        } else if let value = value as? Bool {
            sqlite3_bind_int64(statement, index, value ? 1 : 0)
        } else if let value = value as? NSNumber {
            sqlite3_bind_int64(statement, index, value.int64Value)
        } else {
            bind(statement, index, jsonString(value))
        }
    }

    private static func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
        columnNullableText(statement, index) ?? ""
    }

    private static func columnNullableText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let pointer = sqlite3_column_text(statement, index)
        else {
            return nil
        }
        return String(cString: pointer)
    }

    private static func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    private static func jsonString(_ value: Any) -> String {
        if let object = value as? [String: Any] {
            return jsonBody(object)
        }
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return jsonBody(["value": String(describing: value)])
        }
        return string
    }

    private static func jsonBody(_ object: [String: Any]) -> String {
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return "{}"
        }
        return string
    }
}

private let SQLITE_TRANSIENT_BUDGET_QUARANTINE = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
