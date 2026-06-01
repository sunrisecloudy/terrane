import Foundation
import SQLite3

struct RuntimeCrashRecord {
    let sessionId: String
    let reloadOffered: Bool
    let canAutoRemount: Bool
}

final class RuntimeCrashRecovery {
    private let database: PlatformDatabase

    init(databaseURL: URL? = nil) {
        self.database = PlatformDatabase(databaseURL: databaseURL)
    }

    static func newSessionId() -> String {
        "runtime_macos_webview_\(UUID().uuidString.lowercased())"
    }

    func startRuntimeSession(sessionId: String, activeAppId: String? = nil, activeInstallId: String? = nil) {
        guard let db = database.handle else { return }
        let metadata = jsonBody([
            "source": "native-macos-webview",
            "reloadOffered": false,
            "runtimeReady": false,
        ] as [String: Any])
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, ended_at, status, capabilities_json, metadata_json)
        VALUES (?, 'macos', 'macos', '0.1.0', ?, ?, ?, NULL, 'running', '{}', ?)
        ON CONFLICT(session_id) DO UPDATE SET
          active_app_id = excluded.active_app_id,
          active_install_id = excluded.active_install_id,
          ended_at = NULL,
          status = 'running',
          metadata_json = excluded.metadata_json
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        bindNullable(statement, 2, activeAppId)
        bindNullable(statement, 3, activeInstallId)
        bind(statement, 4, now())
        bind(statement, 5, metadata)
        sqlite3_step(statement)
    }

    func recordWebContentProcessTerminated(
        sessionId: String,
        previousMountCompletedReady: Bool
    ) -> RuntimeCrashRecord {
        guard let db = database.handle else {
            return RuntimeCrashRecord(sessionId: sessionId, reloadOffered: true, canAutoRemount: false)
        }
        let metadata = jsonBody([
            "source": "native-macos-webview",
            "reason": "web_content_process_terminated",
            "reloadOffered": true,
            "canAutoRemount": previousMountCompletedReady,
        ] as [String: Any])
        var statement: OpaquePointer?
        let sql = """
        UPDATE runtime_sessions
        SET ended_at = ?,
            status = 'failed',
            metadata_json = ?
        WHERE session_id = ?
        """
        if sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK {
            defer { sqlite3_finalize(statement) }
            bind(statement, 1, now())
            bind(statement, 2, metadata)
            bind(statement, 3, sessionId)
            sqlite3_step(statement)
        }
        return RuntimeCrashRecord(
            sessionId: sessionId,
            reloadOffered: true,
            canAutoRemount: previousMountCompletedReady
        )
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_RUNTIME_CRASH)
    }

    private func bindNullable(_ statement: OpaquePointer?, _ index: Int32, _ value: String?) {
        guard let value else {
            sqlite3_bind_null(statement, index)
            return
        }
        bind(statement, index, value)
    }

    private func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }

    private func jsonBody(_ object: [String: Any]) -> String {
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return "{}"
        }
        return string
    }
}

private let SQLITE_TRANSIENT_RUNTIME_CRASH = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
