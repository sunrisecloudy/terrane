import Foundation
import SQLite3

final class PlatformStorage {
    private var db: OpaquePointer?

    init() {
        let url = databaseURL()
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        sqlite3_open(url.path, &db)
        sqlite3_exec(db, "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));", nil, nil, nil)
    }

    deinit {
        sqlite3_close(db)
    }

    func get(_ request: BridgeRequest) -> BridgeResponse {
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.get requires key")
        }
        let appId = appId(for: key)
        let sql = "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
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
        let appId = appId(for: key)
        let value = encodeJson(request.params["value"] ?? NSNull())
        let sql = "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        bind(statement, 3, value)
        bind(statement, 4, ISO8601DateFormatter().string(from: Date()))
        sqlite3_step(statement)
        return .success(id: request.id, result: ["ok": true, "bytesWritten": value.utf8.count])
    }

    func remove(_ request: BridgeRequest) -> BridgeResponse {
        guard let key = request.params["key"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.remove requires key")
        }
        let sql = "DELETE FROM app_storage WHERE app_id = ? AND key = ?"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId(for: key))
        bind(statement, 2, key)
        sqlite3_step(statement)
        return .success(id: request.id, result: ["ok": true])
    }

    func list(_ request: BridgeRequest) -> BridgeResponse {
        guard let prefix = request.params["prefix"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "storage.list requires prefix")
        }
        let sql = "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key"
        var statement: OpaquePointer?
        sqlite3_prepare_v2(db, sql, -1, &statement, nil)
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId(for: prefix))
        bind(statement, 2, "\(prefix)%")
        var keys: [String] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            if let text = sqlite3_column_text(statement, 0) {
                keys.append(String(cString: text))
            }
        }
        return .success(id: request.id, result: ["keys": keys])
    }

    private func databaseURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("NativeAIWebappPlatform/platform.sqlite")
    }

    private func appId(for key: String) -> String {
        key.split(separator: ":", maxSplits: 1).first.map(String.init) ?? "unknown"
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
