import Foundation
import CryptoKit
import SQLite3

struct StorageResetResult {
    let snapshotId: String
    let clearedStorageKeys: Int
    let storageRowsDeleted: Int
    let contentHash: String
}

final class PlatformStorage {
    private let database: PlatformDatabase
    private var db: OpaquePointer? { database.handle }
    var databaseHandle: OpaquePointer? { database.handle }

    init(databaseURL: URL? = nil) {
        self.database = PlatformDatabase(databaseURL: databaseURL)
    }

    func get(_ request: BridgeRequest) -> BridgeResponse {
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.get requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        let sql = "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, key)
        if sqlite3_step(statement) == SQLITE_ROW, let text = sqlite3_column_text(statement, 0) {
            let value = String(cString: text)
            return .success(id: request.id, result: ["value": decodeJson(value)])
        }
        return .success(id: request.id, result: ["value": request.params["defaultValue"] ?? NSNull()])
    }

    func set(_ request: BridgeRequest) -> BridgeResponse {
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.set requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        ensureAppRow(request.context.appId)
        let value = encodeJson(request.params["value"] ?? NSNull())
        if let limit = request.context.resourceBudget["maxStorageBytes"] {
            let projectedBytes = storageBytesAfterSet(appId: request.context.appId, key: key, valueBytes: value.utf8.count)
            if projectedBytes > limit {
                return .failure(
                    id: request.id,
                    code: "resource_budget_exceeded",
                    message: "Storage write exceeds manifest.resourceBudget.maxStorageBytes",
                    details: [
                        "appId": request.context.appId,
                        "key": key,
                        "budget": "maxStorageBytes",
                        "current": projectedBytes,
                        "max": limit,
                        "limit": limit,
                        "projectedBytes": projectedBytes
                    ]
                )
            }
        }
        let sql = "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, key)
        bind(statement, 3, value)
        bind(statement, 4, ISO8601DateFormatter().string(from: Date()))
        sqlite3_step(statement)
        return .success(id: request.id, result: ["ok": true, "bytesWritten": value.utf8.count])
    }

    private func storageBytesAfterSet(appId: String, key: String, valueBytes: Int) -> Int {
        let sql = "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ? AND key != ?"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        let currentOtherBytes = sqlite3_step(statement) == SQLITE_ROW ? Int(sqlite3_column_int64(statement, 0)) : 0
        return currentOtherBytes + valueBytes
    }

    func remove(_ request: BridgeRequest) -> BridgeResponse {
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.remove requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        let sql = "DELETE FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, key)
        sqlite3_step(statement)
        return .success(id: request.id, result: ["ok": true])
    }

    func list(_ request: BridgeRequest) -> BridgeResponse {
        guard let prefix = request.params["prefix"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.list requires prefix")
        }
        guard prefix.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: prefix)
        }
        let sql = "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, "\(prefix)%")
        var keys: [String] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            if let text = sqlite3_column_text(statement, 0) {
                keys.append(String(cString: text))
            }
        }
        return .success(id: request.id, result: ["keys": keys])
    }

    func resetAppStorage(appId: String, sessionId: String? = nil, installId: String? = nil) -> StorageResetResult? {
        guard let db else { return nil }
        let validSessionId = sessionId.flatMap { runtimeSessionExists($0) ? $0 : nil }
        let activeInstallId = installId ?? activeInstallId(appId: appId)
        let createdAt = ISO8601DateFormatter().string(from: Date())
        let snapshotId = "snapshot_ios_\(UUID().uuidString.lowercased())"

        guard sqlite3_exec(db, "BEGIN IMMEDIATE", nil, nil, nil) == SQLITE_OK else {
            return nil
        }
        let rows = storageRows(appId: appId)
        let snapshotJSON = stableJsonString([
            "appId": appId,
            "activeInstallId": activeInstallId ?? NSNull(),
            "storage": rows,
            "createdAt": createdAt
        ])
        let contentHash = "sha256:\(sha256Hex(snapshotJSON))"

        let insertSQL = """
        INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at)
        VALUES (?, ?, ?, ?, 'manual', ?, ?, ?)
        """
        var insert: OpaquePointer?
        guard sqlite3_prepare_v2(db, insertSQL, -1, &insert, nil) == SQLITE_OK else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return nil
        }
        bind(insert, 1, snapshotId)
        bindNullable(insert, 2, validSessionId)
        bind(insert, 3, appId)
        bindNullable(insert, 4, activeInstallId)
        bind(insert, 5, snapshotJSON)
        bind(insert, 6, contentHash)
        bind(insert, 7, createdAt)
        guard sqlite3_step(insert) == SQLITE_DONE else {
            sqlite3_finalize(insert)
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return nil
        }
        sqlite3_finalize(insert)

        var delete: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM app_storage WHERE app_id = ?", -1, &delete, nil) == SQLITE_OK else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return nil
        }
        bind(delete, 1, appId)
        guard sqlite3_step(delete) == SQLITE_DONE else {
            sqlite3_finalize(delete)
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return nil
        }
        let deleted = Int(sqlite3_changes(db))
        sqlite3_finalize(delete)

        guard sqlite3_exec(db, "COMMIT", nil, nil, nil) == SQLITE_OK else {
            sqlite3_exec(db, "ROLLBACK", nil, nil, nil)
            return nil
        }
        return StorageResetResult(
            snapshotId: snapshotId,
            clearedStorageKeys: rows.count,
            storageRowsDeleted: deleted,
            contentHash: contentHash
        )
    }

    func runtimeSnapshotExists(snapshotId: String, appId: String) -> Bool {
        let sql = "SELECT COUNT(*) FROM runtime_snapshots WHERE snapshot_id = ? AND app_id = ? AND type = 'manual'"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, snapshotId)
        bind(statement, 2, appId)
        return sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_int(statement, 0) == 1
    }

    private func storagePrefixFailure(_ request: BridgeRequest, key: String) -> BridgeResponse {
        .failure(
            id: request.id,
            code: "permission_denied",
            message: "Storage key must begin with \(request.context.storagePrefix)",
            details: ["key": key, "prefix": request.context.storagePrefix, "appId": request.context.appId]
        )
    }

    private func ensureAppRow(_ appId: String) {
        let sql = "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, ?, ?)"
        let now = ISO8601DateFormatter().string(from: Date())
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, appId)
        bind(statement, 3, now)
        bind(statement, 4, now)
        sqlite3_step(statement)
    }

    private func storageRows(appId: String) -> [[String: Any]] {
        let sql = "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "app_id": columnText(statement, 0),
                "key": columnText(statement, 1),
                "value_json": columnText(statement, 2),
                "updated_at": columnText(statement, 3)
            ])
        }
        return rows
    }

    private func activeInstallId(appId: String) -> String? {
        let sql = "SELECT active_install_id FROM apps WHERE id = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        guard sqlite3_step(statement) == SQLITE_ROW,
              sqlite3_column_type(statement, 0) != SQLITE_NULL
        else {
            return nil
        }
        return columnText(statement, 0)
    }

    private func runtimeSessionExists(_ sessionId: String) -> Bool {
        let sql = "SELECT COUNT(*) FROM runtime_sessions WHERE session_id = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        return sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_int(statement, 0) == 1
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT)
    }

    private func bindNullable(_ statement: OpaquePointer?, _ index: Int32, _ value: String?) {
        guard let value else {
            sqlite3_bind_null(statement, index)
            return
        }
        bind(statement, index, value)
    }

    private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
        guard let text = sqlite3_column_text(statement, index) else { return "" }
        return String(cString: text)
    }
}

private let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

private func encodeJson(_ value: Any) -> String {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value),
          let text = String(data: data, encoding: .utf8)
    else {
        return "null"
    }
    return text
}

private func decodeJson(_ text: String) -> Any {
    guard let data = text.data(using: .utf8),
          let value = try? JSONSerialization.jsonObject(with: data)
    else {
        return NSNull()
    }
    return value
}

private func stableJsonString(_ value: Any) -> String {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
          let text = String(data: data, encoding: .utf8)
    else {
        return "{}"
    }
    return text
}

private func sha256Hex(_ text: String) -> String {
    SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
}
