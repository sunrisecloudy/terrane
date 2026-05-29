#if DEBUG
import Foundation
import Network
import Security
import SQLite3

private let SQLITE_TRANSIENT_CONTROL = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

final class DevControlPlane: @unchecked Sendable {
    struct Configuration {
        var port: UInt16?
        var tokenFileURL: URL
        var databaseURL: URL?
        var tokenOverride: String?

        static func defaultConfiguration() -> Configuration {
            Configuration(
                port: UInt16(ProcessInfo.processInfo.environment["NATIVE_AI_MACOS_CONTROL_PORT"] ?? ""),
                tokenFileURL: defaultTokenFileURL(),
                databaseURL: nil,
                tokenOverride: nil
            )
        }

        private static func defaultTokenFileURL() -> URL {
            let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
            return base.appendingPathComponent("native-ai-webapp/control.token")
        }
    }

    enum ControlError: Error {
        case alreadyStarted
        case randomTokenFailed
        case listenerNotReady
        case portUnavailable
    }

    let token: String
    let tokenFileURL: URL
    let controlSessionId: String

    private let database: PlatformDatabase
    private let queue = DispatchQueue(label: "dev.nativeai.macos.control-plane")
    private var listener: NWListener?
    private var sessionStatus = "running"

    var boundPort: UInt16? {
        listener?.port?.rawValue
    }

    init(configuration: Configuration = .defaultConfiguration()) throws {
        self.token = try configuration.tokenOverride ?? Self.generateToken()
        self.tokenFileURL = configuration.tokenFileURL
        self.controlSessionId = "control_\(UUID().uuidString.lowercased())"
        self.database = PlatformDatabase(databaseURL: configuration.databaseURL)
        try writeTokenFile()
        try createControlSession()
        try configureListener(port: configuration.port)
    }

    static func enabledFromProcess() throws -> DevControlPlane? {
        let args = CommandLine.arguments
        let env = ProcessInfo.processInfo.environment
        guard args.contains("--native-ai-dev-control") || env["NATIVE_AI_MACOS_DEV_CONTROL"] == "1" else {
            return nil
        }
        return try DevControlPlane()
    }

    func start(waitUntilReady: Bool = false) throws {
        guard let listener else { throw ControlError.portUnavailable }
        guard listener.state != .ready else { return }

        let ready = DispatchSemaphore(value: 0)
        let failed = LockedBox<Error?>(nil)
        listener.stateUpdateHandler = { state in
            switch state {
            case .ready:
                ready.signal()
            case let .failed(error):
                failed.value = error
                ready.signal()
            default:
                break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            self?.handle(connection)
        }
        listener.start(queue: queue)

        if waitUntilReady {
            guard ready.wait(timeout: .now() + 2.0) == .success else {
                throw ControlError.listenerNotReady
            }
            if let error = failed.value {
                throw error
            }
        }
    }

    func stop() {
        listener?.cancel()
        markControlSessionEnded()
    }

    private func configureListener(port: UInt16?) throws {
        let parameters = NWParameters.tcp
        let listenPort = NWEndpoint.Port(rawValue: port ?? 0) ?? .any
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(IPv4Address("127.0.0.1")!), port: listenPort)
        listener = try NWListener(using: parameters, on: listenPort)
    }

    private func handle(_ connection: NWConnection) {
        connection.start(queue: queue)
        receiveRequest(on: connection, accumulated: Data())
    }

    private func receiveRequest(on connection: NWConnection, accumulated: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, _, _ in
            guard let self else {
                connection.cancel()
                return
            }
            var requestData = accumulated
            if let data {
                requestData.append(data)
            }
            guard !requestData.isEmpty else {
                self.send(connection, status: 400, body: errorBody("invalid_request", "Control request must not be empty", sessionId: self.controlSessionId))
                return
            }
            guard self.isCompleteHTTPRequest(requestData) else {
                self.receiveRequest(on: connection, accumulated: requestData)
                return
            }
            self.process(requestData, on: connection)
        }
    }

    private func process(_ data: Data, on connection: NWConnection) {
        let startedAt = Date()
        guard let request = String(data: data, encoding: .utf8),
              let parsed = HTTPRequest(request)
        else {
            send(connection, status: 400, body: errorBody("invalid_request", "Control request must be HTTP text", sessionId: controlSessionId))
            return
        }

        guard parsed.headers["x-platform-control-token"] == token else {
            audit(parsed, decision: "rejected", errorCode: "control_auth_required", startedAt: startedAt, result: nil)
            send(connection, status: 401, body: errorBody("control_auth_required", "Control token is required", sessionId: controlSessionId))
            return
        }

        switch (parsed.method, parsed.normalizedPath) {
        case ("GET", "/health"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: healthResult())
        case ("POST", "/sessions"):
            sessionStatus = "running"
            sendAccepted(connection, parsed, startedAt: startedAt, result: sessionResult())
        case ("POST", "/command"):
            handleCommand(connection, parsed, startedAt: startedAt)
        case ("POST", "/db/snapshot"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbSnapshotResult())
        default:
            handleSessionRoute(connection, parsed, startedAt: startedAt)
        }
    }

    private func isCompleteHTTPRequest(_ data: Data) -> Bool {
        guard let raw = String(data: data, encoding: .utf8) else {
            return false
        }
        let normalized = raw.replacingOccurrences(of: "\r\n", with: "\n")
        guard let headerEnd = normalized.range(of: "\n\n") else {
            return false
        }
        let headerLines = normalized[..<headerEnd.lowerBound].split(separator: "\n").map(String.init)
        let contentLengthLine = headerLines.first { $0.lowercased().hasPrefix("content-length:") }
        let contentLength = contentLengthLine
            .flatMap { line in line.split(separator: ":", maxSplits: 1).last }
            .flatMap { Int($0.trimmingCharacters(in: .whitespacesAndNewlines)) } ?? 0
        let body = normalized[headerEnd.upperBound...]
        return body.utf8.count >= contentLength
    }

    private func handleSessionRoute(_ connection: NWConnection, _ request: HTTPRequest, startedAt: Date) {
        guard let route = SessionRoute(request.normalizedPath) else {
            sendRejected(connection, request, status: 404, code: "not_found", message: "Control endpoint was not found", startedAt: startedAt)
            return
        }
        guard route.controlSessionId == controlSessionId else {
            sendRejected(connection, request, status: 404, code: "not_found", message: "Control session was not found", startedAt: startedAt)
            return
        }

        switch (request.method, route.subresource) {
        case ("DELETE", nil):
            sessionStatus = "ended"
            markControlSessionEnded()
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "controlSessionId": controlSessionId,
                "target": "macos",
                "status": sessionStatus,
                "endedAt": Self.now(),
            ])
        case ("GET", "snapshot"):
            sendAccepted(connection, request, startedAt: startedAt, result: snapshotResult())
        case ("GET", "events"):
            sendAccepted(connection, request, startedAt: startedAt, result: eventsResult())
        case ("POST", "command"):
            handleCommand(connection, request, startedAt: startedAt)
        default:
            sendRejected(connection, request, status: 404, code: "not_found", message: "Control session route was not found", startedAt: startedAt)
        }
    }

    private func handleCommand(_ connection: NWConnection, _ request: HTTPRequest, startedAt: Date) {
        guard let body = request.jsonBody else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "Control command body must be JSON", startedAt: startedAt)
            return
        }
        let tool = body["tool"] as? String ?? ""
        switch tool {
        case "platform.health":
            sendAccepted(connection, request, startedAt: startedAt, result: healthResult())
        case "runtime.snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: snapshotResult())
        case "runtime.event_log":
            sendAccepted(connection, request, startedAt: startedAt, result: eventsResult())
        case "db.snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: dbSnapshotResult())
        default:
            sendRejected(
                connection,
                request,
                status: 501,
                code: "platform_unsupported",
                message: "Control command is not implemented by the macOS host yet",
                startedAt: startedAt
            )
        }
    }

    private func sendAccepted(_ connection: NWConnection, _ request: HTTPRequest, startedAt: Date, result: [String: Any]) {
        let body = controlResponse(result: result, sessionId: controlSessionId)
        audit(request, decision: "accepted", errorCode: nil, startedAt: startedAt, result: jsonBody(result))
        send(connection, status: 200, body: body)
    }

    private func sendRejected(
        _ connection: NWConnection,
        _ request: HTTPRequest,
        status: Int,
        code: String,
        message: String,
        startedAt: Date
    ) {
        audit(request, decision: "rejected", errorCode: code, startedAt: startedAt, result: nil)
        send(connection, status: status, body: errorBody(code, message, sessionId: controlSessionId))
    }

    private func healthResult() -> [String: Any] {
        [
            "platform": "macos",
            "target": "macos",
            "devMode": true,
            "controlSessionId": controlSessionId,
            "status": sessionStatus,
        ]
    }

    private func sessionResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "runtimeSessionId": NSNull(),
            "target": "macos",
            "appId": NSNull(),
            "status": sessionStatus,
        ]
    }

    private func snapshotResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "snapshot": [
                "platform": "macos",
                "target": "macos",
                "activeAppId": NSNull(),
                "runtimeAttached": false,
                "controlCommands": controlCommandCount(),
            ],
        ]
    }

    private func eventsResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "runtimeSessionId": NSNull(),
            "appId": NSNull(),
            "bridgeCalls": [],
            "coreEvents": [],
            "controlCommands": controlCommands(),
        ]
    }

    private func dbSnapshotResult() -> [String: Any] {
        [
            "apps": tableRows(
                table: "apps",
                columns: ["id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"],
                orderBy: "id"
            ),
            "app_versions": tableRows(
                table: "app_versions",
                columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "content_hash", "status", "created_at", "activated_at"],
                orderBy: "created_at"
            ),
            "app_storage": tableRows(
                table: "app_storage",
                columns: ["app_id", "key", "value_json", "updated_at"],
                orderBy: "updated_at"
            ),
            "bridge_calls": tableRows(
                table: "bridge_calls",
                columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"],
                orderBy: "created_at"
            ),
            "core_events": tableRows(
                table: "core_events",
                columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
                orderBy: "created_at"
            ),
            "test_runs": tableRows(
                table: "test_runs",
                columns: ["test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"],
                orderBy: "started_at"
            ),
            "control_sessions": tableRows(
                table: "control_sessions",
                columns: ["control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"],
                orderBy: "started_at"
            ),
            "control_commands": tableRows(
                table: "control_commands",
                columns: ["command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "created_at", "duration_ms"],
                orderBy: "created_at"
            ),
            "runtime_sessions": tableRows(
                table: "runtime_sessions",
                columns: ["session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"],
                orderBy: "started_at"
            ),
            "runtime_snapshots": tableRows(
                table: "runtime_snapshots",
                columns: ["snapshot_id", "session_id", "app_id", "install_id", "type", "content_hash", "created_at"],
                orderBy: "created_at"
            ),
        ]
    }

    private func send(_ connection: NWConnection, status: Int, body: String) {
        let reason = statusReason(status)
        let bytes = body.data(using: .utf8) ?? Data()
        let response = """
        HTTP/1.1 \(status) \(reason)\r
        Content-Type: application/json\r
        Content-Length: \(bytes.count)\r
        Connection: close\r
        \r
        \(body)
        """
        connection.send(content: response.data(using: .utf8), completion: .contentProcessed { _ in
            connection.cancel()
        })
    }

    private func writeTokenFile() throws {
        try FileManager.default.createDirectory(
            at: tokenFileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try token.write(to: tokenFileURL, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: tokenFileURL.path)
    }

    private static func generateToken() throws -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            throw ControlError.randomTokenFailed
        }
        return Data(bytes)
            .base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    private func createControlSession() throws {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        let sql = """
        INSERT OR REPLACE INTO control_sessions (control_session_id, target, actor, token_hash, started_at, status, metadata_json)
        VALUES (?, 'macos', 'codex', NULL, ?, 'running', '{"source":"native-macos"}')
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, controlSessionId)
        bind(statement, 2, Self.now())
        sqlite3_step(statement)
    }

    private func markControlSessionEnded() {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "UPDATE control_sessions SET status = 'ended', ended_at = ? WHERE control_session_id = ?", -1, &statement, nil) == SQLITE_OK else {
            return
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, Self.now())
        bind(statement, 2, controlSessionId)
        sqlite3_step(statement)
    }

    private func audit(_ request: HTTPRequest, decision: String, errorCode: String?, startedAt: Date, result: String?) {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO control_commands (command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms)
        VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "command_\(UUID().uuidString.lowercased())")
        bind(statement, 2, controlSessionId)
        bind(statement, 3, request.toolName)
        bind(statement, 4, request.method)
        bind(statement, 5, request.path)
        bind(statement, 6, decision)
        bindNullable(statement, 7, errorCode)
        bindNullable(statement, 8, result)
        bindNullable(statement, 9, errorCode.map { errorBody($0, "Control request rejected", sessionId: controlSessionId) })
        bind(statement, 10, Self.now())
        sqlite3_bind_int64(statement, 11, Int64(Date().timeIntervalSince(startedAt) * 1000))
        sqlite3_step(statement)
    }

    private func controlCommandCount() -> Int {
        guard let db = database.handle else { return 0 }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM control_commands WHERE control_session_id = ?", -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, controlSessionId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return 0
        }
        return Int(sqlite3_column_int(statement, 0))
    }

    private func controlCommands() -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let sql = """
        SELECT command_id, tool, http_method, path, decision, error_code, created_at, duration_ms
        FROM control_commands
        WHERE control_session_id = ?
        ORDER BY created_at, command_id
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, controlSessionId)

        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "commandId": columnText(statement, 0),
                "tool": columnText(statement, 1),
                "method": columnText(statement, 2),
                "path": columnText(statement, 3),
                "decision": columnText(statement, 4),
                "errorCode": columnNullableText(statement, 5) ?? NSNull(),
                "createdAt": columnText(statement, 6),
                "durationMs": Int(sqlite3_column_int64(statement, 7)),
            ])
        }
        return rows
    }

    private func tableRows(table: String, columns: [String], orderBy: String) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let sql = "SELECT \(columns.joined(separator: ", ")) FROM \(table) ORDER BY \(orderBy) LIMIT 100"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }

        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            var row: [String: Any] = [:]
            for (index, column) in columns.enumerated() {
                row[column] = sqliteValue(statement, Int32(index))
            }
            rows.append(row)
        }
        return rows
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_CONTROL)
    }

    private func bindNullable(_ statement: OpaquePointer?, _ index: Int32, _ value: String?) {
        guard let value else {
            sqlite3_bind_null(statement, index)
            return
        }
        bind(statement, index, value)
    }

    private static func now() -> String {
        ISO8601DateFormatter().string(from: Date())
    }
}

private struct HTTPRequest {
    let method: String
    let path: String
    let normalizedPath: String
    let headers: [String: String]
    let body: String

    init?(_ raw: String) {
        let normalized = raw.replacingOccurrences(of: "\r\n", with: "\n")
        let lines = normalized.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard let requestLine = lines.first else { return nil }
        let parts = requestLine.split(separator: " ", maxSplits: 2).map(String.init)
        guard parts.count >= 2 else { return nil }
        self.method = parts[0].uppercased()
        self.path = parts[1].split(separator: "?", maxSplits: 1).first.map(String.init) ?? parts[1]
        if self.path == "/control" {
            self.normalizedPath = "/"
        } else if self.path.hasPrefix("/control/") {
            self.normalizedPath = String(self.path.dropFirst("/control".count))
        } else {
            self.normalizedPath = self.path
        }
        var headers: [String: String] = [:]
        var bodyStartIndex: Int?
        for (index, line) in lines.enumerated().dropFirst() {
            if line.isEmpty {
                bodyStartIndex = index + 1
                break
            }
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = String(line[..<colon]).trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            let value = String(line[line.index(after: colon)...]).trimmingCharacters(in: .whitespacesAndNewlines)
            headers[name] = value
        }
        self.headers = headers
        if let bodyStartIndex, bodyStartIndex < lines.count {
            self.body = lines[bodyStartIndex...].joined(separator: "\n")
        } else {
            self.body = ""
        }
    }

    var toolName: String {
        if normalizedPath == "/command" || normalizedPath.hasSuffix("/command"),
           let tool = jsonBody?["tool"] as? String,
           !tool.isEmpty
        {
            return tool
        }
        switch (method, normalizedPath) {
        case ("GET", "/health"):
            return "platform.health"
        case ("POST", "/sessions"):
            return "platform.launch"
        case ("POST", "/db/snapshot"):
            return "db.snapshot"
        case ("DELETE", _):
            return "platform.stop"
        case ("GET", let value) where value.hasSuffix("/snapshot"):
            return "runtime.snapshot"
        case ("GET", let value) where value.hasSuffix("/events"):
            return "runtime.event_log"
        default:
            return "\(method) \(path)"
        }
    }

    var jsonBody: [String: Any]? {
        guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let data = body.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: data),
              let object = raw as? [String: Any]
        else {
            return nil
        }
        return object
    }
}

private struct SessionRoute {
    let controlSessionId: String
    let subresource: String?

    init?(_ path: String) {
        let parts = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard parts.count >= 2, parts[0] == "sessions" else {
            return nil
        }
        self.controlSessionId = parts[1]
        self.subresource = parts.count > 2 ? parts[2] : nil
    }
}

private final class LockedBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: T

    init(_ value: T) {
        self.storage = value
    }

    var value: T {
        get {
            lock.lock()
            defer { lock.unlock() }
            return storage
        }
        set {
            lock.lock()
            storage = newValue
            lock.unlock()
        }
    }
}

private func controlResponse(result: [String: Any], sessionId: String) -> String {
    jsonBody([
        "ok": true,
        "result": result,
        "diagnostics": diagnostics(sessionId: sessionId),
    ])
}

private func errorBody(_ code: String, _ message: String, sessionId: String) -> String {
    jsonBody([
        "ok": false,
        "error": [
            "code": code,
            "message": message,
            "details": [:],
        ],
        "diagnostics": diagnostics(sessionId: sessionId),
    ])
}

private func diagnostics(sessionId: String) -> [String: Any] {
    [
        "target": "macos",
        "sessionId": sessionId,
        "timestamp": ISO8601DateFormatter().string(from: Date()),
    ]
}

private func jsonBody(_ object: [String: Any]) -> String {
    guard JSONSerialization.isValidJSONObject(object),
          let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
          let string = String(data: data, encoding: .utf8)
    else {
        return #"{"ok":false,"error":{"code":"json_encode_failed","message":"Control response could not be encoded","details":{}},"diagnostics":{}}"#
    }
    return string
}

private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
    columnNullableText(statement, index) ?? ""
}

private func columnNullableText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
    guard sqlite3_column_type(statement, index) != SQLITE_NULL,
          let pointer = sqlite3_column_text(statement, index)
    else {
        return nil
    }
    return String(cString: pointer)
}

private func sqliteValue(_ statement: OpaquePointer?, _ index: Int32) -> Any {
    switch sqlite3_column_type(statement, index) {
    case SQLITE_INTEGER:
        return Int(sqlite3_column_int64(statement, index))
    case SQLITE_FLOAT:
        return sqlite3_column_double(statement, index)
    case SQLITE_TEXT:
        return columnText(statement, index)
    case SQLITE_NULL:
        return NSNull()
    default:
        return columnText(statement, index)
    }
}

private func statusReason(_ status: Int) -> String {
    switch status {
    case 200: return "OK"
    case 400: return "Bad Request"
    case 401: return "Unauthorized"
    case 404: return "Not Found"
    case 501: return "Not Implemented"
    default: return "OK"
    }
}
#endif
