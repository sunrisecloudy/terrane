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
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, _, _ in
            guard let self else {
                connection.cancel()
                return
            }
            let startedAt = Date()
            guard let data,
                  let request = String(data: data, encoding: .utf8),
                  let parsed = HTTPRequest(request)
            else {
                self.send(connection, status: 400, body: errorBody("invalid_request", "Control request must be HTTP text"))
                return
            }

            guard parsed.headers["x-platform-control-token"] == self.token else {
                self.audit(parsed, decision: "rejected", errorCode: "control_auth_required", startedAt: startedAt, result: nil)
                self.send(connection, status: 401, body: errorBody("control_auth_required", "Control token is required"))
                return
            }

            switch (parsed.method, parsed.path) {
            case ("GET", "/health"):
                let body = """
                {"ok":true,"platform":"macos","target":"macos","devMode":true,"controlSessionId":"\(controlSessionId)"}
                """
                self.audit(parsed, decision: "accepted", errorCode: nil, startedAt: startedAt, result: body)
                self.send(connection, status: 200, body: body)
            case ("POST", "/sessions"):
                let body = """
                {"ok":true,"controlSessionId":"\(controlSessionId)","target":"macos","status":"running"}
                """
                self.audit(parsed, decision: "accepted", errorCode: nil, startedAt: startedAt, result: body)
                self.send(connection, status: 200, body: body)
            default:
                let code = parsed.isAllowedControlPath ? "platform_unsupported" : "not_found"
                let status = parsed.isAllowedControlPath ? 501 : 404
                self.audit(parsed, decision: "rejected", errorCode: code, startedAt: startedAt, result: nil)
                self.send(connection, status: status, body: errorBody(code, "Control endpoint is not implemented by the macOS host yet"))
            }
        }
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
        bindNullable(statement, 9, errorCode.map { errorBody($0, "Control request rejected") })
        bind(statement, 10, Self.now())
        sqlite3_bind_int64(statement, 11, Int64(Date().timeIntervalSince(startedAt) * 1000))
        sqlite3_step(statement)
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
    let headers: [String: String]

    init?(_ raw: String) {
        let normalized = raw.replacingOccurrences(of: "\r\n", with: "\n")
        let lines = normalized.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard let requestLine = lines.first else { return nil }
        let parts = requestLine.split(separator: " ", maxSplits: 2).map(String.init)
        guard parts.count >= 2 else { return nil }
        self.method = parts[0].uppercased()
        self.path = parts[1]
        var headers: [String: String] = [:]
        for line in lines.dropFirst() {
            if line.isEmpty { break }
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = String(line[..<colon]).trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            let value = String(line[line.index(after: colon)...]).trimmingCharacters(in: .whitespacesAndNewlines)
            headers[name] = value
        }
        self.headers = headers
    }

    var toolName: String {
        switch (method, path) {
        case ("GET", "/health"):
            return "platform.health"
        case ("POST", "/sessions"):
            return "platform.launch"
        default:
            return "\(method) \(path)"
        }
    }

    var isAllowedControlPath: Bool {
        path == "/health" ||
            path == "/sessions" ||
            path.hasPrefix("/sessions/")
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

private func errorBody(_ code: String, _ message: String) -> String {
    #"{"ok":false,"error":{"code":"\#(code)","message":"\#(message)","details":{}}}"#
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
