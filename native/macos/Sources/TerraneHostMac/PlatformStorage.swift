import Foundation
import SQLite3

final class PlatformStorage {
    private let database: PlatformDatabase
    private var db: OpaquePointer? { database.handle }
    var databaseHandle: OpaquePointer? { database.handle }

    init(databaseURL: URL? = nil) {
        self.database = PlatformDatabase(databaseURL: databaseURL)
    }

    func get(_ request: BridgeRequest) -> BridgeResponse {
        guard let db else {
            return storageError(request, operation: "storage.get")
        }
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.get requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        let appId = request.context.appId
        let sql = "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return storageError(request, operation: "storage.get")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        let status = sqlite3_step(statement)
        if status == SQLITE_ROW, let text = sqlite3_column_text(statement, 0) {
            let value = String(cString: text)
            return .success(id: request.id, result: ["value": decodeJson(value)])
        }
        guard status == SQLITE_DONE else {
            return storageError(request, operation: "storage.get")
        }
        return .success(id: request.id, result: ["value": request.params["defaultValue"] ?? NSNull()])
    }

    func set(_ request: BridgeRequest) -> BridgeResponse {
        guard let db else {
            return storageError(request, operation: "storage.set")
        }
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.set requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        let appId = request.context.appId
        guard ensureAppRow(appId) else {
            return storageError(request, operation: "storage.set")
        }
        let value = encodeJson(request.params["value"] ?? NSNull())
        if let limit = request.context.resourceBudget["maxStorageBytes"] {
            guard let projectedBytes = storageBytesAfterSet(appId: appId, key: key, valueBytes: value.utf8.count) else {
                return storageError(request, operation: "storage.set")
            }
            if projectedBytes > limit {
                return .failure(
                    id: request.id,
                    code: "resource_budget_exceeded",
                    message: "Storage write exceeds manifest.resourceBudget.maxStorageBytes",
                    details: [
                        "appId": appId,
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
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return storageError(request, operation: "storage.set")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        bind(statement, 3, value)
        bind(statement, 4, ISO8601DateFormatter().string(from: Date()))
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return storageError(request, operation: "storage.set")
        }
        return .success(id: request.id, result: ["ok": true, "bytesWritten": value.utf8.count])
    }

    private func storageBytesAfterSet(appId: String, key: String, valueBytes: Int) -> Int? {
        guard let db else { return nil }
        let sql = "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ? AND key != ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        let currentOtherBytes = Int(sqlite3_column_int64(statement, 0))
        return currentOtherBytes + valueBytes
    }

    func remove(_ request: BridgeRequest) -> BridgeResponse {
        guard let db else {
            return storageError(request, operation: "storage.remove")
        }
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.remove requires key")
        }
        guard key.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: key)
        }
        let sql = "DELETE FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return storageError(request, operation: "storage.remove")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, key)
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return storageError(request, operation: "storage.remove")
        }
        return .success(id: request.id, result: ["ok": true])
    }

    func list(_ request: BridgeRequest) -> BridgeResponse {
        guard let db else {
            return storageError(request, operation: "storage.list")
        }
        guard let prefix = request.params["prefix"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.list requires prefix")
        }
        guard prefix.hasPrefix(request.context.storagePrefix) else {
            return storagePrefixFailure(request, key: prefix)
        }
        let sql = "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return storageError(request, operation: "storage.list")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, request.context.appId)
        bind(statement, 2, "\(prefix)%")
        var keys: [String] = []
        var status = sqlite3_step(statement)
        while status == SQLITE_ROW {
            if let text = sqlite3_column_text(statement, 0) {
                keys.append(String(cString: text))
            }
            status = sqlite3_step(statement)
        }
        guard status == SQLITE_DONE else {
            return storageError(request, operation: "storage.list")
        }
        return .success(id: request.id, result: ["keys": keys])
    }

    private func storagePrefixFailure(_ request: BridgeRequest, key: String) -> BridgeResponse {
        .failure(
            id: request.id,
            code: "permission_denied",
            message: "Storage key must begin with \(request.context.storagePrefix)",
            details: ["key": key, "prefix": request.context.storagePrefix, "appId": request.context.appId]
        )
    }

    private func storageError(_ request: BridgeRequest, operation: String) -> BridgeResponse {
        .failure(
            id: request.id,
            code: "storage_error",
            message: "\(operation) failed",
            details: ["operation": operation, "appId": request.context.appId]
        )
    }

    private func ensureAppRow(_ appId: String) -> Bool {
        guard let db else { return false }
        let sql = "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, ?, ?)"
        let now = ISO8601DateFormatter().string(from: Date())
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, appId)
        bind(statement, 3, now)
        bind(statement, 4, now)
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT)
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
