#if DEBUG && targetEnvironment(simulator)
import CryptoKit
import Foundation
import Network
import Security
import SQLite3

final class IOSDevControlPlane: @unchecked Sendable {
    struct Configuration {
        var port: UInt16?
        var tokenFileURL: URL
        var tokenOverride: String?

        static func defaultConfiguration() -> Configuration {
            let env = ProcessInfo.processInfo.environment
            return Configuration(
                port: port(from: CommandLine.arguments, env: env),
                tokenFileURL: tokenFileURL(from: env),
                tokenOverride: env["PLATFORM_CONTROL_TOKEN"]
            )
        }

        private static func port(from args: [String], env: [String: String]) -> UInt16? {
            if let index = args.firstIndex(of: "--control-plane-port"),
               args.indices.contains(args.index(after: index)) {
                return UInt16(args[args.index(after: index)])
            }
            return UInt16(env["NATIVE_AI_IOS_CONTROL_PORT"] ?? "")
        }

        private static func tokenFileURL(from env: [String: String]) -> URL {
            if let path = env["PLATFORM_CONTROL_TOKEN_FILE"], !path.isEmpty {
                return URL(fileURLWithPath: path)
            }
            let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first ??
                FileManager.default.temporaryDirectory
            return base
                .appendingPathComponent("native-ai-webapp")
                .appendingPathComponent("control.token")
        }
    }

    enum ControlError: Error {
        case randomTokenFailed
        case listenerUnavailable
    }

    let token: String
    let tokenHash: String
    let tokenFileURL: URL
    let controlSessionId: String

    private let queue = DispatchQueue(label: "dev.nativeai.ios.control-plane")
    private let database: PlatformDatabase
    private var listener: NWListener?

    var boundPort: UInt16? {
        listener?.port?.rawValue
    }

    init(configuration: Configuration = .defaultConfiguration()) throws {
        token = try configuration.tokenOverride ?? Self.generateToken()
        tokenHash = Self.sha256Hex(token)
        tokenFileURL = configuration.tokenFileURL
        controlSessionId = "control_ios_\(UUID().uuidString.lowercased())"
        database = PlatformDatabase()
        try writeTokenFile()
        try configureListener(port: configuration.port)
        createControlSession()
    }

    deinit {
        stop()
    }

    static func enabledFromProcess() throws -> IOSDevControlPlane? {
        let env = ProcessInfo.processInfo.environment
        guard CommandLine.arguments.contains("--native-ai-dev-control") || env["NATIVE_AI_IOS_DEV_CONTROL"] == "1" else {
            return nil
        }
        return try IOSDevControlPlane()
    }

    func start() {
        guard let listener else { return }
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.emitReadyMarker()
            case let .failed(error):
                print("NATIVE_AI_IOS_CONTROL_FAILED \(error)")
                fflush(stdout)
            default:
                break
            }
        }
        listener.newConnectionHandler = { [weak self] connection in
            self?.handle(connection)
        }
        listener.start(queue: queue)
    }

    func stop() {
        listener?.cancel()
        listener = nil
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
                self.send(connection, status: 400, body: self.errorBody("invalid_request", "Control request must not be empty"))
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
        guard let requestText = String(data: data, encoding: .utf8),
              let request = HTTPRequest(requestText)
        else {
            let body = errorBody("invalid_request", "Control request must be HTTP text")
            audit(
                tool: "ios.dev_control",
                method: nil,
                path: nil,
                decision: "rejected",
                errorCode: "invalid_request",
                startedAt: startedAt,
                result: nil,
                error: body
            )
            send(connection, status: 400, body: body)
            return
        }

        guard request.headers["x-platform-control-token"] == token else {
            let body = errorBody("control_auth_required", "Control token is required")
            audit(
                request: request,
                decision: "rejected",
                errorCode: "control_auth_required",
                startedAt: startedAt,
                result: nil,
                error: body
            )
            send(connection, status: 401, body: body)
            return
        }

        if request.method == "GET" && request.normalizedPath == "/health" {
            let body: [String: Any] = [
                "ok": true,
                "target": "ios-simulator",
                "platform": "ios",
                "sessionId": controlSessionId,
                "controlPlane": [
                    "port": Int(boundPort ?? 0),
                    "tokenPath": tokenFileURL.path,
                    "auth": "token-file",
                    "loopback": true
                ]
            ]
            audit(
                request: request,
                decision: "accepted",
                errorCode: nil,
                startedAt: startedAt,
                result: body,
                error: nil
            )
            send(connection, status: 200, body: body)
            return
        }

        let body = errorBody("not_found", "iOS dev control route is not supported")
        audit(
            request: request,
            decision: "rejected",
            errorCode: "not_found",
            startedAt: startedAt,
            result: nil,
            error: body
        )
        send(connection, status: 404, body: body)
    }

    private func send(_ connection: NWConnection, status: Int, body: [String: Any]) {
        let payload = (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data(#"{"ok":false}"#.utf8)
        let reason = status == 200 ? "OK" : status == 401 ? "Unauthorized" : status == 404 ? "Not Found" : "Bad Request"
        var response = "HTTP/1.1 \(status) \(reason)\r\n"
        response += "Content-Type: application/json\r\n"
        response += "Content-Length: \(payload.count)\r\n"
        response += "Connection: close\r\n\r\n"
        var data = Data(response.utf8)
        data.append(payload)
        connection.send(content: data, completion: .contentProcessed { _ in
            connection.cancel()
        })
    }

    private func errorBody(_ code: String, _ message: String) -> [String: Any] {
        [
            "ok": false,
            "error": [
                "code": code,
                "message": message,
                "details": [:]
            ],
            "diagnostics": [
                "sessionId": controlSessionId,
                "target": "ios-simulator"
            ]
        ]
    }

    private func isCompleteHTTPRequest(_ data: Data) -> Bool {
        guard let text = String(data: data, encoding: .utf8),
              let headerRange = text.range(of: "\r\n\r\n")
        else {
            return false
        }
        let headerText = String(text[..<headerRange.lowerBound])
        let contentLength = headerText
            .components(separatedBy: "\r\n")
            .compactMap { line -> Int? in
                let parts = line.split(separator: ":", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
                guard parts.count == 2, parts[0].lowercased() == "content-length" else { return nil }
                return Int(parts[1])
            }
            .first ?? 0
        let headerBytes = text[..<headerRange.upperBound].utf8.count
        return data.count >= headerBytes + contentLength
    }

    private func writeTokenFile() throws {
        try FileManager.default.createDirectory(at: tokenFileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(token.utf8).write(to: tokenFileURL, options: [.atomic])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: tokenFileURL.path)
    }

    private func createControlSession() {
        guard let db = database.handle else { return }
        let sql = """
        INSERT OR REPLACE INTO control_sessions (control_session_id, target, actor, token_hash, started_at, status, metadata_json)
        VALUES (?, 'ios-simulator', 'codex', ?, ?, 'running', ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, controlSessionId)
        bind(statement, 2, tokenHash)
        bind(statement, 3, Self.now())
        bind(statement, 4, jsonString([
            "source": "native-ios",
            "surface": "dev-control-health",
            "tokenPath": tokenFileURL.path,
            "loopback": true
        ]))
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

    private func audit(
        request: HTTPRequest,
        decision: String,
        errorCode: String?,
        startedAt: Date,
        result: [String: Any]?,
        error: [String: Any]?
    ) {
        audit(
            tool: request.toolName,
            method: request.method,
            path: request.normalizedPath,
            decision: decision,
            errorCode: errorCode,
            startedAt: startedAt,
            result: result,
            error: error
        )
    }

    private func audit(
        tool: String,
        method: String?,
        path: String?,
        decision: String,
        errorCode: String?,
        startedAt: Date,
        result: [String: Any]?,
        error: [String: Any]?
    ) {
        guard let db = database.handle else { return }
        let sql = """
        INSERT INTO control_commands (command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "command_ios_\(UUID().uuidString.lowercased())")
        bind(statement, 2, controlSessionId)
        bind(statement, 3, tool)
        bindNullable(statement, 4, method)
        bindNullable(statement, 5, path)
        bind(statement, 6, decision)
        bindNullable(statement, 7, errorCode)
        bind(statement, 8, jsonString(["path": path ?? ""]))
        bindNullable(statement, 9, result.map { jsonString($0) })
        bindNullable(statement, 10, error.map { jsonString($0) })
        bind(statement, 11, Self.now())
        sqlite3_bind_int64(statement, 12, Int64(Date().timeIntervalSince(startedAt) * 1000))
        sqlite3_step(statement)
    }

    private func emitReadyMarker() {
        print("NATIVE_AI_IOS_CONTROL_READY port=\(boundPort ?? 0) tokenPath=\(tokenFileURL.path)")
        fflush(stdout)
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_IOS_CONTROL)
    }

    private func bindNullable(_ statement: OpaquePointer?, _ index: Int32, _ value: String?) {
        guard let value else {
            sqlite3_bind_null(statement, index)
            return
        }
        bind(statement, index, value)
    }

    private func jsonString(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return "{}"
        }
        return text
    }

    private static func now() -> String {
        ISO8601DateFormatter().string(from: Date())
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

    private static func sha256Hex(_ text: String) -> String {
        let digest = SHA256.hash(data: Data(text.utf8))
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    private struct HTTPRequest {
        let method: String
        let path: String
        let headers: [String: String]

        var normalizedPath: String {
            path.split(separator: "?", maxSplits: 1).first.map(String.init) ?? path
        }

        var toolName: String {
            method == "GET" && normalizedPath == "/health" ? "platform.health" : "ios.dev_control"
        }

        init?(_ request: String) {
            let head = request.components(separatedBy: "\r\n\r\n").first ?? request
            let lines = head.components(separatedBy: "\r\n")
            guard let requestLine = lines.first else { return nil }
            let parts = requestLine.split(separator: " ")
            guard parts.count >= 2 else { return nil }
            method = String(parts[0]).uppercased()
            path = String(parts[1])
            var parsedHeaders: [String: String] = [:]
            for line in lines.dropFirst() {
                let pieces = line.split(separator: ":", maxSplits: 1)
                guard pieces.count == 2 else { continue }
                parsedHeaders[String(pieces[0]).lowercased()] = pieces[1].trimmingCharacters(in: .whitespaces)
            }
            headers = parsedHeaders
        }
    }
}

private let SQLITE_TRANSIENT_IOS_CONTROL = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
#endif
