#if DEBUG && targetEnvironment(simulator)
import CryptoKit
import Foundation
import Network
import Security
import SQLite3

final class IOSDevControlPlane: @unchecked Sendable {
    typealias BridgeCommandHandler = (String, String, [String: Any], String?) async -> BridgeResponse

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
    private let bridgeCommandHandler: BridgeCommandHandler?
    private var listener: NWListener?

    var boundPort: UInt16? {
        listener?.port?.rawValue
    }

    init(configuration: Configuration = .defaultConfiguration(), bridgeCommandHandler: BridgeCommandHandler? = nil) throws {
        token = try configuration.tokenOverride ?? Self.generateToken()
        tokenHash = Self.sha256Hex(token)
        tokenFileURL = configuration.tokenFileURL
        controlSessionId = "control_ios_\(UUID().uuidString.lowercased())"
        database = PlatformDatabase()
        self.bridgeCommandHandler = bridgeCommandHandler
        try writeTokenFile()
        try configureListener(port: configuration.port)
        createControlSession()
    }

    deinit {
        stop()
    }

    @MainActor
    static func enabledFromProcess(bridge: WebBridge) throws -> IOSDevControlPlane? {
        let env = ProcessInfo.processInfo.environment
        guard CommandLine.arguments.contains("--native-ai-dev-control") || env["NATIVE_AI_IOS_DEV_CONTROL"] == "1" else {
            return nil
        }
        return try IOSDevControlPlane { appId, method, params, id in
            await bridge.handleControlBridgeCall(appId: appId, method: method, params: params, id: id)
        }
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

        if request.method == "POST" && isSessionCreatePath(request.normalizedPath) {
            let body = createRuntimeSession(request)
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

        if request.method == "DELETE",
           let sessionId = sessionId(from: request.normalizedPath) {
            let body = endRuntimeSession(sessionId)
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

        if request.method == "GET",
           let sessionId = sessionId(from: request.normalizedPath),
           request.normalizedPath.hasSuffix("/snapshot") {
            let body = runtimeSnapshot(sessionId: sessionId)
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

        if request.method == "GET",
           let sessionId = sessionId(from: request.normalizedPath),
           request.normalizedPath.hasSuffix("/events") {
            let body = runtimeEvents(sessionId: sessionId)
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

        if request.method == "GET",
           let sessionId = sessionId(from: request.normalizedPath),
           request.normalizedPath.hasSuffix("/capabilities") {
            let body = successBody(result: runtimeCapabilities(appId: activeAppId(sessionId: sessionId) ?? "notes-lite"), sessionId: sessionId)
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

        if request.method == "POST" && isCommandPath(request.normalizedPath) {
            Task { [weak self] in
                guard let self else {
                    connection.cancel()
                    return
                }
                do {
                    let body = try await self.dispatchCommand(request)
                    self.queue.async {
                        self.audit(
                            tool: request.commandToolName ?? request.toolName,
                            method: request.method,
                            path: request.normalizedPath,
                            decision: "accepted",
                            errorCode: nil,
                            startedAt: startedAt,
                            result: body,
                            error: nil
                        )
                        self.send(connection, status: 200, body: body)
                    }
                } catch let commandError as CommandError {
                    let body = self.errorBody(commandError.code, commandError.message, details: commandError.details)
                    self.queue.async {
                        self.audit(
                            tool: request.commandToolName ?? request.toolName,
                            method: request.method,
                            path: request.normalizedPath,
                            decision: "rejected",
                            errorCode: commandError.code,
                            startedAt: startedAt,
                            result: nil,
                            error: body
                        )
                        self.send(connection, status: commandError.status, body: body)
                    }
                } catch {
                    let body = self.errorBody("invalid_request", "Control command must be valid JSON")
                    self.queue.async {
                        self.audit(
                            tool: request.toolName,
                            method: request.method,
                            path: request.normalizedPath,
                            decision: "rejected",
                            errorCode: "invalid_request",
                            startedAt: startedAt,
                            result: nil,
                            error: body
                        )
                        self.send(connection, status: 400, body: body)
                    }
                }
            }
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
        let reason = status == 200 ? "OK" : status == 401 ? "Unauthorized" : status == 403 ? "Forbidden" : status == 404 ? "Not Found" : status == 503 ? "Service Unavailable" : "Bad Request"
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

    private func errorBody(_ code: String, _ message: String, details: [String: Any] = [:]) -> [String: Any] {
        [
            "ok": false,
            "error": [
                "code": code,
                "message": message,
                "details": details
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

    private func isSessionCreatePath(_ path: String) -> Bool {
        path == "/sessions" || path == "/control/sessions"
    }

    private func isCommandPath(_ path: String) -> Bool {
        path == "/command" || path == "/control/command" || path.hasSuffix("/command")
    }

    private func sessionId(from path: String) -> String? {
        for prefix in ["/sessions/", "/control/sessions/"] {
            guard path.hasPrefix(prefix) else { continue }
            let rest = path.dropFirst(prefix.count)
            guard let id = rest.split(separator: "/").first, !id.isEmpty else {
                return nil
            }
            return String(id)
        }
        return nil
    }

    private func createRuntimeSession(_ request: HTTPRequest) -> [String: Any] {
        let requestBody = request.jsonObject() ?? [:]
        let appId = appId(from: requestBody)
        let runtimeSessionId = "runtime_ios_control_\(UUID().uuidString.lowercased())"
        let capabilities = runtimeCapabilities(appId: appId)
        insertRuntimeSession(sessionId: runtimeSessionId, appId: appId, capabilities: capabilities)
        return successBody(result: [
            "sessionId": controlSessionId,
            "runtimeSessionId": runtimeSessionId,
            "target": "ios-simulator",
            "platform": "ios",
            "appId": appId ?? NSNull(),
            "capabilities": capabilities
        ], sessionId: controlSessionId)
    }

    private func endRuntimeSession(_ sessionId: String) -> [String: Any] {
        if sessionId == controlSessionId {
            markControlSessionEnded()
            return successBody(result: ["sessionId": sessionId, "status": "ended"], sessionId: sessionId)
        }
        guard let db = database.handle else {
            return successBody(result: ["sessionId": sessionId, "status": "ended"], sessionId: sessionId)
        }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "UPDATE runtime_sessions SET status = 'ended', ended_at = ? WHERE session_id = ?", -1, &statement, nil) == SQLITE_OK else {
            return successBody(result: ["sessionId": sessionId, "status": "ended"], sessionId: sessionId)
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, Self.now())
        bind(statement, 2, sessionId)
        sqlite3_step(statement)
        return successBody(result: ["sessionId": sessionId, "status": "ended"], sessionId: sessionId)
    }

    private func runtimeSnapshot(sessionId: String) -> [String: Any] {
        successBody(result: [
            "sessionId": sessionId,
            "target": "ios-simulator",
            "platform": "ios",
            "runtimeLoaded": true,
            "activeAppId": activeAppId(sessionId: sessionId) ?? NSNull(),
            "webapps": bundledWebapps()
        ], sessionId: sessionId)
    }

    private func runtimeEvents(sessionId: String) -> [String: Any] {
        successBody(result: [
            "sessionId": sessionId,
            "controlCommands": controlCommandRows(limit: 100),
            "bridgeCalls": bridgeCallRows(limit: 100),
            "coreEvents": coreEventRows(limit: 100),
            "coreActions": coreActionRows(limit: 100)
        ], sessionId: sessionId)
    }

    private func dispatchCommand(_ request: HTTPRequest) async throws -> [String: Any] {
        guard let command = request.jsonObject(),
              let tool = command["tool"] as? String
        else {
            throw CommandError(status: 400, code: "invalid_request", message: "Control command requires a string tool")
        }
        let args = command["args"] as? [String: Any] ?? [:]
        let sessionId = request.routeSessionId ?? (args["sessionId"] as? String) ?? controlSessionId

        switch tool {
        case "platform.health":
            return successBody(result: [
                "target": "ios-simulator",
                "platform": "ios",
                "controlSessionId": controlSessionId,
                "controlPlane": [
                    "port": Int(boundPort ?? 0),
                    "tokenPath": tokenFileURL.path,
                    "loopback": true
                ]
            ], sessionId: sessionId)
        case "platform.list_targets":
            return successBody(result: platformListTargets(), sessionId: sessionId)
        case "platform.list_webapps":
            return successBody(result: [
                "source": "ios-bundled",
                "apps": bundledWebapps()
            ], sessionId: sessionId)
        case "runtime.capabilities":
            let appId = (args["appId"] as? String) ?? activeAppId(sessionId: sessionId) ?? "notes-lite"
            return try await bridgeCommandBody(
                appId: appId,
                method: "runtime.capabilities",
                params: [:],
                id: "control_capabilities",
                sessionId: sessionId
            )
        case "runtime.call_bridge":
            guard let method = args["method"] as? String, !method.isEmpty else {
                throw CommandError(status: 400, code: "invalid_request", message: "runtime.call_bridge requires method")
            }
            let appId = (args["appId"] as? String) ?? activeAppId(sessionId: sessionId) ?? "notes-lite"
            let params = args["params"] as? [String: Any] ?? [:]
            return try await bridgeCommandBody(
                appId: appId,
                method: method,
                params: params,
                id: (args["id"] as? String) ?? "control_call_bridge",
                sessionId: sessionId
            )
        case "runtime.core_step":
            let appId = (args["appId"] as? String) ?? activeAppId(sessionId: sessionId) ?? "task-workbench"
            var params = args
            params.removeValue(forKey: "appId")
            params.removeValue(forKey: "sessionId")
            return try await bridgeCommandBody(
                appId: appId,
                method: "core.step",
                params: params,
                id: (args["id"] as? String) ?? "control_core_step",
                sessionId: sessionId
            )
        default:
            throw CommandError(
                status: 404,
                code: "platform_unsupported",
                message: "iOS dev control tool is not implemented yet",
                details: ["tool": tool]
            )
        }
    }

    private func platformListTargets() -> [String: Any] {
        [
            "targets": [[
                "id": "ios-simulator",
                "platform": "ios",
                "status": "running",
                "controlSessionId": controlSessionId,
                "controlPlane": [
                    "port": Int(boundPort ?? 0),
                    "tokenPath": tokenFileURL.path,
                    "loopback": true
                ],
                "tools": [
                    "platform.health",
                    "platform.list_targets",
                    "platform.list_webapps",
                    "runtime.capabilities"
                ]
            ]]
        ]
    }

    private func bridgeCommandBody(
        appId: String,
        method: String,
        params: [String: Any],
        id: String?,
        sessionId: String
    ) async throws -> [String: Any] {
        guard let bridgeCommandHandler else {
            throw CommandError(
                status: 503,
                code: "platform_unsupported",
                message: "iOS dev control bridge routing is not available"
            )
        }
        let response = await bridgeCommandHandler(appId, method, params, id)
        if response.ok {
            return successBody(result: [
                "bridgeResponse": response.asDictionary()
            ], sessionId: sessionId)
        }
        let error = response.error ?? [
            "code": "bridge_error",
            "message": "Bridge command failed",
            "details": [:]
        ]
        throw CommandError(
            status: 400,
            code: (error["code"] as? String) ?? "bridge_error",
            message: (error["message"] as? String) ?? "Bridge command failed",
            details: [
                "appId": appId,
                "method": method,
                "bridgeError": error
            ]
        )
    }

    private func bundledWebapps() -> [[String: Any]] {
        guard let index = try? JSONSerialization.jsonObject(with: BundledAppCatalog.appIndexData()) as? [String: Any],
              let apps = index["apps"] as? [[String: Any]]
        else {
            return []
        }
        return apps.map { app in
            var normalized = app
            normalized["source"] = "bundled"
            normalized["status"] = "available"
            return normalized
        }
    }

    private func runtimeCapabilities(appId: String?) -> [String: Any] {
        let selectedAppId = appId ?? "notes-lite"
        var limits: [String: Any] = [
            "maxPackageBytes": 1_048_576,
            "maxFileBytes": 524_288
        ]
        for (key, value) in resourceBudget(appId: selectedAppId) {
            limits[key] = value
        }
        return [
            "platform": "ios",
            "target": "ios-simulator",
            "appId": selectedAppId,
            "runtimeVersion": "0.1.0",
            "devMode": true,
            "features": [
                "storage.read": true,
                "storage.write": true,
                "storage.get": true,
                "storage.set": true,
                "storage.remove": true,
                "storage.list": true,
                "dialog.openFile": true,
                "dialog.saveFile": true,
                "notification.toast": true,
                "network.request": true,
                "core.step": ZigCoreBridge().isAvailable,
                "runtime.capabilities": true,
                "app.log": true
            ],
            "limits": limits
        ]
    }

    private func successBody(result: [String: Any], sessionId: String? = nil) -> [String: Any] {
        [
            "ok": true,
            "result": result,
            "diagnostics": [
                "target": "ios-simulator",
                "sessionId": sessionId ?? controlSessionId,
                "timestamp": Self.now()
            ]
        ]
    }

    private func appId(from body: [String: Any]) -> String? {
        if let appId = body["appId"] as? String, !appId.isEmpty {
            return appId
        }
        if let args = body["args"] as? [String: Any],
           let appId = args["appId"] as? String,
           !appId.isEmpty {
            return appId
        }
        return nil
    }

    private func manifest(appId: String) -> [String: Any] {
        guard let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return [:]
        }
        return json
    }

    private func resourceBudget(appId: String) -> [String: Int] {
        guard let budget = manifest(appId: appId)["resourceBudget"] as? [String: Any] else { return [:] }
        var normalized: [String: Int] = [:]
        for (key, value) in budget {
            if let intValue = value as? Int {
                normalized[key] = intValue
            } else if let number = value as? NSNumber {
                normalized[key] = number.intValue
            }
        }
        return normalized
    }

    private func insertRuntimeSession(sessionId: String, appId: String?, capabilities: [String: Any]) {
        guard let db = database.handle else { return }
        let sql = """
        INSERT OR REPLACE INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json)
        VALUES (?, 'ios-simulator', 'ios', '0.1.0', ?, NULL, ?, 'running', ?, ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        bindNullable(statement, 2, appId)
        bind(statement, 3, Self.now())
        bind(statement, 4, jsonString(capabilities))
        bind(statement, 5, jsonString(["source": "ios-dev-control"]))
        sqlite3_step(statement)
    }

    private func activeAppId(sessionId: String) -> String? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT active_app_id FROM runtime_sessions WHERE session_id = ?", -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        guard sqlite3_step(statement) == SQLITE_ROW,
              sqlite3_column_type(statement, 0) != SQLITE_NULL
        else {
            return nil
        }
        return columnText(statement, 0)
    }

    private func controlCommandRows(limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let sql = """
        SELECT command_id, tool, http_method, path, decision, error_code, created_at, duration_ms
        FROM control_commands
        WHERE control_session_id = ?
        ORDER BY created_at DESC
        LIMIT ?
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, controlSessionId)
        sqlite3_bind_int64(statement, 2, Int64(limit))
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "commandId": columnText(statement, 0),
                "tool": columnText(statement, 1),
                "method": columnText(statement, 2),
                "path": columnText(statement, 3),
                "decision": columnText(statement, 4),
                "errorCode": nullableColumnText(statement, 5) ?? NSNull(),
                "createdAt": columnText(statement, 6),
                "durationMs": Int(sqlite3_column_int64(statement, 7))
            ])
        }
        return rows
    }

    private func bridgeCallRows(limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let sql = """
        SELECT bridge_call_id, session_id, app_id, method, result_json, error_json, duration_ms, created_at
        FROM bridge_calls
        ORDER BY created_at DESC
        LIMIT ?
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_int64(statement, 1, Int64(limit))
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "bridgeCallId": columnText(statement, 0),
                "sessionId": columnText(statement, 1),
                "appId": nullableColumnText(statement, 2) ?? NSNull(),
                "method": columnText(statement, 3),
                "resultJson": nullableColumnText(statement, 4) ?? NSNull(),
                "errorJson": nullableColumnText(statement, 5) ?? NSNull(),
                "durationMs": Int(sqlite3_column_int64(statement, 6)),
                "createdAt": columnText(statement, 7)
            ])
        }
        return rows
    }

    private func coreEventRows(limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let sql = """
        SELECT event_id, session_id, app_id, event_json, created_at
        FROM core_events
        ORDER BY created_at DESC
        LIMIT ?
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_int64(statement, 1, Int64(limit))
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "eventId": columnText(statement, 0),
                "sessionId": nullableColumnText(statement, 1) ?? NSNull(),
                "appId": nullableColumnText(statement, 2) ?? NSNull(),
                "eventJson": columnText(statement, 3),
                "createdAt": columnText(statement, 4)
            ])
        }
        return rows
    }

    private func coreActionRows(limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let sql = """
        SELECT action_id, event_id, session_id, app_id, action_json, created_at
        FROM core_actions
        ORDER BY created_at DESC
        LIMIT ?
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_int64(statement, 1, Int64(limit))
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "actionId": columnText(statement, 0),
                "eventId": columnText(statement, 1),
                "sessionId": nullableColumnText(statement, 2) ?? NSNull(),
                "appId": nullableColumnText(statement, 3) ?? NSNull(),
                "actionJson": columnText(statement, 4),
                "createdAt": columnText(statement, 5)
            ])
        }
        return rows
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

    private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let text = sqlite3_column_text(statement, index)
        else {
            return ""
        }
        return String(cString: text)
    }

    private func nullableColumnText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let text = sqlite3_column_text(statement, index)
        else {
            return nil
        }
        return String(cString: text)
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

    private struct CommandError: Error {
        let status: Int
        let code: String
        let message: String
        let details: [String: Any]

        init(status: Int, code: String, message: String, details: [String: Any] = [:]) {
            self.status = status
            self.code = code
            self.message = message
            self.details = details
        }
    }

    private struct HTTPRequest {
        let method: String
        let path: String
        let headers: [String: String]
        let body: String

        var normalizedPath: String {
            path.split(separator: "?", maxSplits: 1).first.map(String.init) ?? path
        }

        var toolName: String {
            if method == "GET" && normalizedPath == "/health" {
                return "platform.health"
            }
            if normalizedPath == "/command" || normalizedPath == "/control/command" {
                return commandToolName ?? "control.command"
            }
            if normalizedPath.hasPrefix("/sessions") || normalizedPath.hasPrefix("/control/sessions") {
                if method == "POST" && normalizedPath.hasSuffix("/command") {
                    return commandToolName ?? "control.sessions.command"
                }
                if method == "POST" {
                    return "control.sessions.create"
                }
                if method == "DELETE" {
                    return "control.sessions.end"
                }
                if normalizedPath.hasSuffix("/snapshot") {
                    return "control.sessions.snapshot"
                }
                if normalizedPath.hasSuffix("/events") {
                    return "control.sessions.events"
                }
                if normalizedPath.hasSuffix("/capabilities") {
                    return "control.sessions.capabilities"
                }
                return "control.sessions"
            }
            return "ios.dev_control"
        }

        var commandToolName: String? {
            jsonObject()?["tool"] as? String
        }

        var routeSessionId: String? {
            for prefix in ["/sessions/", "/control/sessions/"] {
                guard normalizedPath.hasPrefix(prefix) else { continue }
                let rest = normalizedPath.dropFirst(prefix.count)
                guard let id = rest.split(separator: "/").first, !id.isEmpty else {
                    return nil
                }
                return String(id)
            }
            return nil
        }

        func jsonObject() -> [String: Any]? {
            guard !body.isEmpty,
                  let data = body.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else {
                return nil
            }
            return object
        }

        init?(_ request: String) {
            let partsByHeader = request.components(separatedBy: "\r\n\r\n")
            let head = partsByHeader.first ?? request
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
            body = partsByHeader.dropFirst().joined(separator: "\r\n\r\n")
        }
    }
}

private let SQLITE_TRANSIENT_IOS_CONTROL = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
#endif
