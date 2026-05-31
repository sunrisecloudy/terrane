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

        if request.method == "POST",
           let dbTool = dbToolName(forPath: request.normalizedPath) {
            do {
                let args = (request.jsonObject()?["args"] as? [String: Any]) ?? request.jsonObject() ?? [:]
                let body = try dispatchDbTool(dbTool, args: args, sessionId: controlSessionId)
                audit(
                    tool: dbTool,
                    method: request.method,
                    path: request.normalizedPath,
                    decision: "accepted",
                    errorCode: nil,
                    startedAt: startedAt,
                    result: body,
                    error: nil
                )
                send(connection, status: 200, body: body)
            } catch let commandError as CommandError {
                let body = errorBody(commandError.code, commandError.message, details: commandError.details)
                audit(
                    tool: dbTool,
                    method: request.method,
                    path: request.normalizedPath,
                    decision: "rejected",
                    errorCode: commandError.code,
                    startedAt: startedAt,
                    result: nil,
                    error: body
                )
                send(connection, status: commandError.status, body: body)
            } catch {
                let body = errorBody("storage_error", "iOS DB control request failed")
                audit(
                    tool: dbTool,
                    method: request.method,
                    path: request.normalizedPath,
                    decision: "rejected",
                    errorCode: "storage_error",
                    startedAt: startedAt,
                    result: nil,
                    error: body
                )
                send(connection, status: 500, body: body)
            }
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
        let reason = status == 200 ? "OK" : status == 401 ? "Unauthorized" : status == 403 ? "Forbidden" : status == 404 ? "Not Found" : status == 500 ? "Internal Server Error" : status == 503 ? "Service Unavailable" : "Bad Request"
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
        case "runtime.storage_get":
            let appId = try requiredString(args, key: "appId", message: "runtime.storage_get requires appId and key")
            return try await bridgeCommandBody(
                appId: appId,
                method: "storage.get",
                params: try storageGetParams(args),
                id: (args["id"] as? String) ?? "control_storage_get",
                sessionId: sessionId
            )
        case "runtime.storage_set":
            let appId = try requiredString(args, key: "appId", message: "runtime.storage_set requires appId, key, and value")
            return try await bridgeCommandBody(
                appId: appId,
                method: "storage.set",
                params: try storageSetParams(args),
                id: (args["id"] as? String) ?? "control_storage_set",
                sessionId: sessionId
            )
        case "runtime.assert_storage":
            return try await runtimeAssertStorage(args: args, sessionId: sessionId)
        case "runtime.storage_reset", "platform.reset_webapp":
            return try runtimeStorageReset(tool: tool, args: args, sessionId: sessionId)
        case "runtime.resource_usage":
            let appId = try requiredString(args, key: "appId", message: "runtime.resource_usage requires appId")
            return successBody(result: resourceUsage(appId: appId), sessionId: sessionId)
        case "runtime.event_log":
            return successBody(result: eventLog(appId: args["appId"] as? String, limit: limit(from: args)), sessionId: sessionId)
        case "runtime.console_logs":
            return successBody(result: consoleLogs(appId: args["appId"] as? String, limit: limit(from: args)), sessionId: sessionId)
        case "runtime.bridge_calls":
            return successBody(result: [
                "appId": (args["appId"] as? String) ?? NSNull(),
                "bridgeCalls": detailedBridgeCallRows(appId: args["appId"] as? String, method: nil, limit: limit(from: args))
            ], sessionId: sessionId)
        case "runtime.clear_logs":
            return successBody(result: try clearRuntimeLogs(appId: args["appId"] as? String), sessionId: sessionId)
        case "runtime.notification_capture":
            return successBody(result: notificationCapture(appId: args["appId"] as? String, limit: limit(from: args)), sessionId: sessionId)
        case "runtime.assert_bridge_call":
            return try assertBridgeCall(args: args, sessionId: sessionId)
        case "runtime.assert_no_console_errors":
            return try assertNoConsoleErrors(args: args, sessionId: sessionId)
        case "runtime.core_snapshot":
            return successBody(result: try coreSnapshot(args: args), sessionId: sessionId)
        case "runtime.replay_events":
            return successBody(result: try replayEvents(args: args), sessionId: sessionId)
        case "runtime.assert_core_action":
            return successBody(result: try assertCoreAction(args: args), sessionId: sessionId)
        case "platform.create_snapshot":
            return successBody(result: try createSnapshot(args: args), sessionId: sessionId)
        case "platform.restore_snapshot":
            return successBody(result: try restoreSnapshot(args: args), sessionId: sessionId)
        case "runtime.compare_snapshot":
            return successBody(result: try compareSnapshot(args: args), sessionId: sessionId)
        case "db.snapshot",
             "db.query_app_storage",
             "db.query_app_versions",
             "db.query_bridge_calls",
             "db.query_core_events",
             "db.query_test_runs",
             "db.export_debug_bundle":
            return try dispatchDbTool(tool, args: args, sessionId: sessionId)
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
                    "runtime.capabilities",
                    "runtime.call_bridge",
                    "runtime.core_step",
                    "runtime.storage_get",
                    "runtime.storage_set",
                    "runtime.assert_storage",
                    "runtime.storage_reset",
                    "platform.reset_webapp",
                    "runtime.resource_usage",
                    "runtime.event_log",
                    "runtime.console_logs",
                    "runtime.bridge_calls",
                    "runtime.clear_logs",
                    "runtime.notification_capture",
                    "runtime.assert_bridge_call",
                    "runtime.assert_no_console_errors",
                    "runtime.core_snapshot",
                    "runtime.replay_events",
                    "runtime.assert_core_action",
                    "platform.create_snapshot",
                    "platform.restore_snapshot",
                    "runtime.compare_snapshot",
                    "db.snapshot",
                    "db.query_app_storage",
                    "db.query_app_versions",
                    "db.query_bridge_calls",
                    "db.query_core_events",
                    "db.query_test_runs",
                    "db.export_debug_bundle"
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
        let response = try await bridgeCommandResponse(appId: appId, method: method, params: params, id: id)
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

    private func bridgeCommandResponse(
        appId: String,
        method: String,
        params: [String: Any],
        id: String?
    ) async throws -> BridgeResponse {
        guard let bridgeCommandHandler else {
            throw CommandError(
                status: 503,
                code: "platform_unsupported",
                message: "iOS dev control bridge routing is not available"
            )
        }
        return await bridgeCommandHandler(appId, method, params, id)
    }

    private func storageGetParams(_ args: [String: Any]) throws -> [String: Any] {
        var params: [String: Any] = [
            "key": try requiredString(args, key: "key", message: "runtime.storage_get requires appId and key")
        ]
        if args.keys.contains("defaultValue") {
            params["defaultValue"] = args["defaultValue"] ?? NSNull()
        }
        return params
    }

    private func storageSetParams(_ args: [String: Any]) throws -> [String: Any] {
        guard args.keys.contains("value") else {
            throw CommandError(status: 400, code: "invalid_request", message: "runtime.storage_set requires appId, key, and value")
        }
        return [
            "key": try requiredString(args, key: "key", message: "runtime.storage_set requires appId, key, and value"),
            "value": args["value"] ?? NSNull()
        ]
    }

    private func runtimeAssertStorage(args: [String: Any], sessionId: String) async throws -> [String: Any] {
        guard args.keys.contains("value") else {
            throw CommandError(status: 400, code: "invalid_request", message: "runtime.assert_storage requires appId, key, and value")
        }
        let appId = try requiredString(args, key: "appId", message: "runtime.assert_storage requires appId, key, and value")
        let key = try requiredString(args, key: "key", message: "runtime.assert_storage requires appId, key, and value")
        let response = try await bridgeCommandResponse(
            appId: appId,
            method: "storage.get",
            params: ["key": key],
            id: (args["id"] as? String) ?? "control_storage_assert_get"
        )
        if !response.ok {
            let error = response.error ?? [
                "code": "assertion_failed",
                "message": "Storage assertion read failed",
                "details": [:]
            ]
            throw CommandError(
                status: 400,
                code: (error["code"] as? String) ?? "assertion_failed",
                message: (error["message"] as? String) ?? "Storage assertion read failed",
                details: ["appId": appId, "key": key, "bridgeError": error]
            )
        }
        guard let result = response.result as? [String: Any] else {
            throw CommandError(status: 400, code: "assertion_failed", message: "Storage assertion read returned an invalid result", details: ["appId": appId, "key": key])
        }
        let actual = result["value"] ?? NSNull()
        let expected = args["value"] ?? NSNull()
        guard jsonValuesEqual(actual, expected) else {
            throw CommandError(
                status: 400,
                code: "assertion_failed",
                message: "Storage value did not match expected value",
                details: ["appId": appId, "key": key, "actual": actual, "expected": expected]
            )
        }
        return successBody(result: [
            "ok": true,
            "appId": appId,
            "key": key,
            "value": actual
        ], sessionId: sessionId)
    }

    private func runtimeStorageReset(tool: String, args: [String: Any], sessionId: String) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "\(tool) requires appId")
        guard args["confirm"] as? Bool == true else {
            throw CommandError(status: 400, code: "confirmation_required", message: "\(tool) requires confirm: true")
        }
        guard let reset = PlatformStorage().resetAppStorage(appId: appId, sessionId: sessionId) else {
            throw CommandError(status: 500, code: "storage_error", message: "Webapp storage could not be reset", details: ["appId": appId])
        }
        var result: [String: Any] = [
            "ok": true,
            "appId": appId,
            "snapshotId": reset.snapshotId,
            "clearedStorageKeys": reset.clearedStorageKeys,
            "storageRowsDeleted": reset.storageRowsDeleted,
            "contentHash": reset.contentHash
        ]
        if tool == "platform.reset_webapp" {
            result["clearedLogs"] = try clearRuntimeLogs(appId: appId)
        }
        return successBody(result: result, sessionId: sessionId)
    }

    private func resourceUsage(appId: String) -> [String: Any] {
        [
            "appId": appId,
            "storageKeys": scalarInt("SELECT COUNT(*) FROM app_storage WHERE app_id = ?", appId: appId),
            "storageBytes": scalarInt("SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?", appId: appId),
            "bridgeCalls": scalarInt("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?", appId: appId),
            "coreEvents": scalarInt("SELECT COUNT(*) FROM core_events WHERE app_id = ?", appId: appId),
            "coreActions": scalarInt("SELECT COUNT(*) FROM core_actions WHERE app_id = ?", appId: appId),
            "networkRequestsLastMinute": scalarInt("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'network.request' AND datetime(created_at) >= datetime('now', '-60 seconds')", appId: appId),
            "logLinesLastMinute": scalarInt("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'app.log' AND datetime(created_at) >= datetime('now', '-60 seconds')", appId: appId)
        ]
    }

    private func eventLog(appId: String?, limit: Int) -> [String: Any] {
        [
            "appId": appId ?? NSNull(),
            "bridgeCalls": detailedBridgeCallRows(appId: appId, method: nil, limit: limit),
            "coreEvents": safeTableRows(Self.safeDbCoreEvents, appId: appId, limit: limit),
            "coreActions": safeTableRows(Self.safeDbCoreActions, appId: appId, limit: limit)
        ]
    }

    private func consoleLogs(appId: String?, limit: Int) -> [String: Any] {
        [
            "appId": appId ?? NSNull(),
            "logs": effectRows(method: "app.log", appId: appId, limit: limit).map { row in
                let params = row["params"] as? [String: Any] ?? [:]
                var normalized = row
                normalized["level"] = params["level"] ?? NSNull()
                normalized["message"] = params["message"] ?? NSNull()
                return normalized
            }
        ]
    }

    private func notificationCapture(appId: String?, limit: Int) -> [String: Any] {
        [
            "appId": appId ?? NSNull(),
            "notifications": effectRows(method: "notification.toast", appId: appId, limit: limit).map { row in
                let params = row["params"] as? [String: Any] ?? [:]
                var normalized = row
                normalized["message"] = params["message"] ?? NSNull()
                normalized["level"] = params["level"] ?? NSNull()
                return normalized
            }
        ]
    }

    private func assertBridgeCall(args: [String: Any], sessionId: String) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "runtime.assert_bridge_call requires appId and method")
        let method = try requiredString(args, key: "method", message: "runtime.assert_bridge_call requires appId and method")
        let rows = detailedBridgeCallRows(appId: appId, method: method, limit: limit(from: args))
        guard let latest = rows.last else {
            throw CommandError(status: 400, code: "assertion_failed", message: "Expected bridge call was not recorded", details: ["appId": appId, "method": method])
        }
        return successBody(result: [
            "ok": true,
            "appId": appId,
            "method": method,
            "count": rows.count,
            "latest": latest
        ], sessionId: sessionId)
    }

    private func assertNoConsoleErrors(args: [String: Any], sessionId: String) throws -> [String: Any] {
        let logs = consoleLogs(appId: args["appId"] as? String, limit: limit(from: args))["logs"] as? [[String: Any]] ?? []
        let errors = logs.filter { row in
            (row["level"] as? String) == "error" || !(row["error"] is NSNull)
        }
        guard errors.isEmpty else {
            throw CommandError(status: 400, code: "console_errors_found", message: "Console error logs were found", details: ["errors": errors])
        }
        return successBody(result: ["ok": true, "errors": 0], sessionId: sessionId)
    }

    private func coreSnapshot(args: [String: Any]) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "runtime.core_snapshot requires appId")
        return [
            "appId": appId,
            "stateVersion": scalarInt("SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?", appId: appId),
            "coreEvents": parsedCoreRows(Self.safeDbCoreEvents, appId: appId, limit: limit(from: args), jsonColumn: "event_json", parsedKey: "event"),
            "coreActions": parsedCoreRows(Self.safeDbCoreActions, appId: appId, limit: limit(from: args), jsonColumn: "action_json", parsedKey: "action")
        ]
    }

    private func replayEvents(args: [String: Any]) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "runtime.replay_events requires appId")
        guard let events = args["events"] as? [Any] else {
            throw CommandError(status: 400, code: "invalid_request", message: "runtime.replay_events events must be an array")
        }
        let replayCore = ZigCoreBridge()
        let replay = events.enumerated().map { index, event -> [String: Any] in
            let request = BridgeRequest(
                body: [
                    "id": "control_replay_\(index)",
                    "method": "core.step",
                    "params": ["event": event],
                    "timestamp": Date().timeIntervalSince1970
                ],
                context: AppSandboxContext(controlAppId: appId, mountToken: "ios-dev-control-replay")
            )
            let response = replayCore.step(request)
            return [
                "index": index,
                "event": event,
                "result": response.ok ? (response.result ?? NSNull()) : [
                    "ok": false,
                    "error": response.error ?? [
                        "code": "core_error",
                        "message": "Replay event failed",
                        "details": [:]
                    ],
                    "actions": []
                ]
            ]
        }
        return [
            "ok": true,
            "appId": appId,
            "replay": replay
        ]
    }

    private func assertCoreAction(args: [String: Any]) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "runtime.assert_core_action requires appId")
        let expectedType = try optionalString(args, key: "type", message: "runtime.assert_core_action type must be a string") ??
            optionalString(args, key: "actionType", message: "runtime.assert_core_action type must be a string")
        let expectedMatch = try optionalDictionary(args, key: "match", message: "runtime.assert_core_action match must be an object")
        let expectedAction = args.keys.contains("action") ? args["action"] ?? NSNull() : nil
        let rows = parsedCoreRows(Self.safeDbCoreActions, appId: appId, limit: limit(from: args), jsonColumn: "action_json", parsedKey: "action")
        var matches: [[String: Any]] = []
        var latest: [String: Any]?
        var latestAction: Any?

        for row in rows {
            let action = row["action"] ?? jsonValue(row["action_json"] as? String)
            if let expectedType {
                guard let actionObject = action as? [String: Any],
                      actionObject["type"] as? String == expectedType else {
                    continue
                }
            }
            if let expectedAction, !jsonValuesEqual(action, expectedAction) {
                continue
            }
            if let expectedMatch, !jsonMatchesSubset(action, expectedMatch) {
                continue
            }
            matches.append(row)
            latest = row
            latestAction = action
        }

        guard !matches.isEmpty else {
            throw CommandError(status: 400, code: "core_action.not_found", message: "Expected core action was not found", details: ["appId": appId, "type": expectedType ?? NSNull()])
        }
        return [
            "ok": true,
            "appId": appId,
            "type": expectedType ?? NSNull(),
            "count": matches.count,
            "actions": matches.map { $0["action"] ?? NSNull() },
            "latest": latest ?? NSNull(),
            "action": latestAction ?? NSNull()
        ]
    }

    private func createSnapshot(args: [String: Any]) throws -> [String: Any] {
        let appId = try requiredString(args, key: "appId", message: "platform.create_snapshot requires appId")
        try requireBundledApp(appId, message: "platform.create_snapshot appId is not a valid generated app id")
        let type = (args["type"] as? String) ?? "manual"
        guard Self.snapshotTypes.contains(type) else {
            throw CommandError(status: 400, code: "invalid_request", message: "Snapshot type is not allowed")
        }
        let metadata = activeAppMetadata(appId: appId)
        let createdAt = Self.now()
        let storage = appStorageRows(appId: appId)
        let snapshotId = "snapshot_ios_\(UUID().uuidString.lowercased())"
        let snapshot: [String: Any] = [
            "appId": appId,
            "activeInstallId": metadata.activeInstallId ?? NSNull(),
            "activeVersion": metadata.activeVersion ?? NSNull(),
            "dataVersion": metadata.dataVersion,
            "storage": storage,
            "createdAt": createdAt
        ]
        let snapshotJson = jsonString(snapshot)
        let contentHash = "sha256:\(Self.sha256Hex(snapshotJson))"
        guard insertRuntimeSnapshot(
            snapshotId: snapshotId,
            sessionId: validRuntimeSessionId(args["sessionId"] as? String),
            appId: appId,
            installId: metadata.activeInstallId,
            type: type,
            snapshotJson: snapshotJson,
            contentHash: contentHash,
            createdAt: createdAt
        ) else {
            throw CommandError(status: 500, code: "storage_error", message: "Snapshot could not be created", details: ["appId": appId])
        }
        return [
            "snapshotId": snapshotId,
            "contentHash": contentHash,
            "snapshot": snapshot,
            "appId": appId,
            "activeInstallId": metadata.activeInstallId ?? NSNull(),
            "activeVersion": metadata.activeVersion ?? NSNull(),
            "dataVersion": metadata.dataVersion,
            "storage": storage,
            "createdAt": createdAt
        ]
    }

    private func restoreSnapshot(args: [String: Any]) throws -> [String: Any] {
        guard args["confirm"] as? Bool == true else {
            throw CommandError(status: 400, code: "confirmation_required", message: "platform.restore_snapshot requires confirm: true")
        }
        let snapshotId = try requiredString(args, key: "snapshotId", message: "platform.restore_snapshot requires snapshotId")
        let record = try readSnapshot(snapshotId: snapshotId)
        guard let snapshot = record["snapshot"] as? [String: Any] else {
            throw CommandError(status: 400, code: "invalid_request", message: "Runtime snapshot JSON is invalid")
        }
        guard let appId = snapshot["appId"] as? String, !appId.isEmpty else {
            throw CommandError(status: 400, code: "invalid_request", message: "Runtime snapshot requires appId")
        }
        let storage = snapshot["storage"] as? [[String: Any]] ?? snapshot["appStorage"] as? [[String: Any]] ?? []
        let now = Self.now()
        try beginTransaction()
        do {
            try ensureAppRow(appId: appId, snapshot: snapshot, updatedAt: now)
            try deleteAppStorage(appId: appId)
            var restored = 0
            for row in storage {
                try restoreStorageRow(row, fallbackAppId: appId, updatedAt: now)
                restored += 1
            }
            try commitTransaction()
            return [
                "ok": true,
                "snapshotId": snapshotId,
                "appId": appId,
                "restoredStorageKeys": restored
            ]
        } catch {
            rollbackTransaction()
            throw error
        }
    }

    private func compareSnapshot(args: [String: Any]) throws -> [String: Any] {
        let left = try snapshotArgument(args: args, valueKey: "left", snapshotIdKey: "leftSnapshotId")
        let right = try snapshotArgument(args: args, valueKey: "right", snapshotIdKey: "rightSnapshotId")
        let leftJson = jsonString(comparableSnapshotValue(left))
        let rightJson = jsonString(comparableSnapshotValue(right))
        let equal = leftJson == rightJson
        return [
            "ok": equal,
            "equal": equal,
            "leftHash": "sha256:\(Self.sha256Hex(leftJson))",
            "rightHash": "sha256:\(Self.sha256Hex(rightJson))"
        ]
    }

    private func clearRuntimeLogs(appId: String?) throws -> [String: Any] {
        let scoped = appId?.isEmpty == false
        return [
            "ok": true,
            "appId": appId ?? NSNull(),
            "coreActionsCleared": try deleteRows(sql: scoped ? "DELETE FROM core_actions WHERE app_id = ?" : "DELETE FROM core_actions", appId: appId),
            "coreEventsCleared": try deleteRows(sql: scoped ? "DELETE FROM core_events WHERE app_id = ?" : "DELETE FROM core_events", appId: appId),
            "bridgeCallsCleared": try deleteRows(sql: scoped ? "DELETE FROM bridge_calls WHERE app_id = ?" : "DELETE FROM bridge_calls", appId: appId)
        ]
    }

    private func deleteRows(sql: String, appId: String?) throws -> Int {
        guard let db = database.handle else {
            throw CommandError(status: 500, code: "storage_error", message: "Platform database is not available")
        }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not prepare runtime log cleanup")
        }
        defer { sqlite3_finalize(statement) }
        if let appId, !appId.isEmpty {
            bind(statement, 1, appId)
        }
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not clear runtime logs")
        }
        return Int(sqlite3_changes(db))
    }

    private func requireBundledApp(_ appId: String, message: String) throws {
        if BundledAppCatalog.denialReason(appId: appId) != nil {
            throw CommandError(status: 400, code: "invalid_request", message: message, details: ["appId": appId])
        }
    }

    private func activeAppMetadata(appId: String) -> ActiveAppMetadata {
        guard let db = database.handle else {
            return ActiveAppMetadata(activeInstallId: nil, activeVersion: manifest(appId: appId)["version"] as? String, dataVersion: dataVersion(appId: appId))
        }
        let sql = "SELECT active_install_id, active_version, data_version FROM apps WHERE id = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return ActiveAppMetadata(activeInstallId: nil, activeVersion: manifest(appId: appId)["version"] as? String, dataVersion: dataVersion(appId: appId))
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        if sqlite3_step(statement) == SQLITE_ROW {
            return ActiveAppMetadata(
                activeInstallId: nullableColumnText(statement, 0),
                activeVersion: nullableColumnText(statement, 1),
                dataVersion: sqlite3_column_type(statement, 2) == SQLITE_NULL ? dataVersion(appId: appId) : Int(sqlite3_column_int64(statement, 2))
            )
        }
        return ActiveAppMetadata(activeInstallId: nil, activeVersion: manifest(appId: appId)["version"] as? String, dataVersion: dataVersion(appId: appId))
    }

    private func dataVersion(appId: String) -> Int {
        if let value = manifest(appId: appId)["dataVersion"] as? Int {
            return value
        }
        if let value = manifest(appId: appId)["dataVersion"] as? NSNumber {
            return value.intValue
        }
        return 1
    }

    private func appStorageRows(appId: String) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
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

    private func insertRuntimeSnapshot(
        snapshotId: String,
        sessionId: String?,
        appId: String,
        installId: String?,
        type: String,
        snapshotJson: String,
        contentHash: String,
        createdAt: String
    ) -> Bool {
        guard let db = database.handle else { return false }
        let sql = """
        INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, snapshotId)
        bindNullable(statement, 2, sessionId)
        bind(statement, 3, appId)
        bindNullable(statement, 4, installId)
        bind(statement, 5, type)
        bind(statement, 6, snapshotJson)
        bind(statement, 7, contentHash)
        bind(statement, 8, createdAt)
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func validRuntimeSessionId(_ sessionId: String?) -> String? {
        guard let sessionId, !sessionId.isEmpty else {
            return nil
        }
        return scalarInt("SELECT COUNT(*) FROM runtime_sessions WHERE session_id = ?", appId: sessionId) > 0 ? sessionId : nil
    }

    private func readSnapshot(snapshotId: String) throws -> [String: Any] {
        guard let db = database.handle else {
            throw CommandError(status: 500, code: "storage_error", message: "Platform database is not available")
        }
        let sql = "SELECT snapshot_id, snapshot_json, content_hash, created_at FROM runtime_snapshots WHERE snapshot_id = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not prepare snapshot read")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, snapshotId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            throw CommandError(status: 404, code: "snapshot_not_found", message: "Runtime snapshot not found: \(snapshotId)")
        }
        guard let snapshot = jsonValue(columnText(statement, 1)) as? [String: Any] else {
            throw CommandError(status: 400, code: "invalid_request", message: "Runtime snapshot JSON is invalid")
        }
        return [
            "snapshotId": columnText(statement, 0),
            "snapshot": snapshot,
            "contentHash": columnText(statement, 2),
            "createdAt": columnText(statement, 3)
        ]
    }

    private func beginTransaction() throws {
        guard executeStatement("BEGIN IMMEDIATE") else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not begin snapshot restore transaction")
        }
    }

    private func commitTransaction() throws {
        guard executeStatement("COMMIT") else {
            _ = executeStatement("ROLLBACK")
            throw CommandError(status: 500, code: "storage_error", message: "Could not commit snapshot restore transaction")
        }
    }

    private func rollbackTransaction() {
        _ = executeStatement("ROLLBACK")
    }

    private func executeStatement(_ sql: String) -> Bool {
        guard let db = database.handle else { return false }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func ensureAppRow(appId: String, snapshot: [String: Any], updatedAt: String) throws {
        guard let db = database.handle else {
            throw CommandError(status: 500, code: "storage_error", message: "Platform database is not available")
        }
        let dataVersion = intValue(snapshot["dataVersion"]) ?? 1
        let sql = """
        INSERT INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at)
        VALUES (?, ?, 'enabled', ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET active_install_id = excluded.active_install_id, active_version = excluded.active_version, data_version = excluded.data_version, status = 'enabled', updated_at = excluded.updated_at
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not prepare app snapshot restore")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, appId)
        bindNullable(statement, 3, snapshot["activeInstallId"] as? String)
        bindNullable(statement, 4, snapshot["activeVersion"] as? String)
        sqlite3_bind_int64(statement, 5, Int64(dataVersion))
        bind(statement, 6, updatedAt)
        bind(statement, 7, updatedAt)
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not restore app snapshot metadata")
        }
    }

    private func deleteAppStorage(appId: String) throws {
        _ = try deleteRows(sql: "DELETE FROM app_storage WHERE app_id = ?", appId: appId)
    }

    private func restoreStorageRow(_ row: [String: Any], fallbackAppId: String, updatedAt: String) throws {
        let rowAppId = row["app_id"] as? String ?? row["appId"] as? String ?? fallbackAppId
        guard !rowAppId.isEmpty,
              let key = row["key"] as? String,
              !key.isEmpty
        else {
            throw CommandError(status: 400, code: "invalid_request", message: "Snapshot storage row requires app_id and key")
        }
        guard rowAppId == fallbackAppId else {
            throw CommandError(status: 400, code: "invalid_request", message: "Snapshot storage row app_id does not match snapshot appId")
        }
        guard key.hasPrefix("\(rowAppId):") else {
            throw CommandError(status: 400, code: "invalid_request", message: "Snapshot storage key is outside app storage prefix")
        }
        guard let db = database.handle else {
            throw CommandError(status: 500, code: "storage_error", message: "Platform database is not available")
        }
        let sql = """
        INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at)
        VALUES (?, ?, ?, ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not prepare snapshot storage restore")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, rowAppId)
        bind(statement, 2, key)
        bind(statement, 3, storageSnapshotValueJson(row))
        bind(statement, 4, updatedAt)
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw CommandError(status: 500, code: "storage_error", message: "Snapshot storage row could not be restored")
        }
    }

    private func storageSnapshotValueJson(_ row: [String: Any]) -> String {
        if let raw = row["value_json"] as? String ?? row["valueJson"] as? String {
            return raw
        }
        return jsonFragmentString(row["value"] ?? NSNull())
    }

    private func snapshotArgument(args: [String: Any], valueKey: String, snapshotIdKey: String) throws -> Any {
        if let snapshotId = args[snapshotIdKey] as? String, !snapshotId.isEmpty {
            return try readSnapshot(snapshotId: snapshotId)["snapshot"] ?? NSNull()
        }
        if let value = args[valueKey], !(value is NSNull) {
            return value
        }
        throw CommandError(status: 400, code: "invalid_request", message: "runtime.compare_snapshot requires left/right snapshots or snapshot ids")
    }

    private func comparableSnapshotValue(_ value: Any) -> Any {
        guard var snapshot = value as? [String: Any] else {
            return value
        }
        snapshot.removeValue(forKey: "createdAt")
        snapshot.removeValue(forKey: "snapshotId")
        if snapshot["storage"] == nil, let appStorage = snapshot["appStorage"] {
            snapshot["storage"] = appStorage
        }
        snapshot.removeValue(forKey: "appStorage")
        if let storage = snapshot["storage"] as? [[String: Any]] {
            snapshot["storage"] = storage.map { row in
                var stable = row
                stable.removeValue(forKey: "updated_at")
                stable.removeValue(forKey: "updatedAt")
                return stable
            }.sorted { left, right in
                storageSortKey(left) < storageSortKey(right)
            }
        }
        return snapshot
    }

    private func storageSortKey(_ row: [String: Any]) -> String {
        let appId = row["app_id"] as? String ?? row["appId"] as? String ?? ""
        let key = row["key"] as? String ?? ""
        return "\(appId)|\(key)"
    }

    private func scalarInt(_ sql: String, appId: String) -> Int {
        guard let db = database.handle else { return 0 }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        return sqlite3_step(statement) == SQLITE_ROW ? Int(sqlite3_column_int64(statement, 0)) : 0
    }

    private func effectRows(method: String, appId: String?, limit: Int) -> [[String: Any]] {
        detailedBridgeCallRows(appId: appId, method: method, limit: limit)
    }

    private func detailedBridgeCallRows(appId: String?, method: String?, limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let base = """
        SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at
        FROM bridge_calls
        """
        let boundedLimit = max(1, min(limit, 500))
        let sql: String
        let values: [String]
        switch (appId?.isEmpty == false ? appId : nil, method?.isEmpty == false ? method : nil) {
        case let (.some(appId), .some(method)):
            sql = "\(base) WHERE app_id = ? AND method = ? ORDER BY created_at LIMIT ?"
            values = [appId, method]
        case let (.some(appId), .none):
            sql = "\(base) WHERE app_id = ? ORDER BY created_at LIMIT ?"
            values = [appId]
        case let (.none, .some(method)):
            sql = "\(base) WHERE method = ? ORDER BY created_at LIMIT ?"
            values = [method]
        case (.none, .none):
            sql = "\(base) ORDER BY created_at LIMIT ?"
            values = []
        }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bind(statement, Int32(index + 1), value)
        }
        sqlite3_bind_int64(statement, Int32(values.count + 1), Int64(boundedLimit))

        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "bridgeCallId": columnText(statement, 0),
                "sessionId": nullableColumnText(statement, 1) ?? NSNull(),
                "appId": nullableColumnText(statement, 2) ?? NSNull(),
                "installId": nullableColumnText(statement, 3) ?? NSNull(),
                "method": columnText(statement, 4),
                "params": jsonValue(nullableColumnText(statement, 5)),
                "result": jsonValue(nullableColumnText(statement, 6)),
                "error": jsonValue(nullableColumnText(statement, 7)),
                "durationMs": sqlite3_column_type(statement, 8) == SQLITE_NULL ? NSNull() : NSNumber(value: sqlite3_column_int64(statement, 8)),
                "createdAt": columnText(statement, 9)
            ])
        }
        return rows
    }

    private func requiredString(_ args: [String: Any], key: String, message: String) throws -> String {
        guard let value = args[key] as? String, !value.isEmpty else {
            throw CommandError(status: 400, code: "invalid_request", message: message)
        }
        return value
    }

    private func optionalString(_ args: [String: Any], key: String, message: String) throws -> String? {
        guard args.keys.contains(key), !(args[key] is NSNull) else {
            return nil
        }
        guard let value = args[key] as? String else {
            throw CommandError(status: 400, code: "invalid_request", message: message)
        }
        return value.isEmpty ? nil : value
    }

    private func optionalDictionary(_ args: [String: Any], key: String, message: String) throws -> [String: Any]? {
        guard args.keys.contains(key), !(args[key] is NSNull) else {
            return nil
        }
        guard let value = args[key] as? [String: Any] else {
            throw CommandError(status: 400, code: "invalid_request", message: message)
        }
        return value
    }

    private func jsonValue(_ text: String?) -> Any {
        guard let text,
              !text.isEmpty,
              let data = text.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(with: data)
        else {
            return NSNull()
        }
        return value
    }

    private func intValue(_ value: Any?) -> Int? {
        if let value = value as? Int {
            return value
        }
        if let value = value as? NSNumber {
            return value.intValue
        }
        return nil
    }

    private func parsedCoreRows(_ spec: SafeDbTable, appId: String, limit: Int, jsonColumn: String, parsedKey: String) -> [[String: Any]] {
        safeTableRows(spec, appId: appId, limit: limit).map { row in
            var copy = row
            copy[parsedKey] = jsonValue(row[jsonColumn] as? String)
            return copy
        }
    }

    private func jsonValuesEqual(_ lhs: Any, _ rhs: Any) -> Bool {
        canonicalJsonString(lhs) == canonicalJsonString(rhs)
    }

    private func jsonMatchesSubset(_ actual: Any, _ expected: Any) -> Bool {
        if expected is NSNull {
            return actual is NSNull
        }
        if let expectedObject = expected as? [String: Any] {
            guard let actualObject = actual as? [String: Any] else {
                return false
            }
            for (key, expectedValue) in expectedObject {
                guard let actualValue = actualObject[key],
                      jsonMatchesSubset(actualValue, expectedValue)
                else {
                    return false
                }
            }
            return true
        }
        return jsonValuesEqual(actual, expected)
    }

    private func canonicalJsonString(_ value: Any) -> String {
        let wrapped: [String: Any] = ["value": value]
        guard JSONSerialization.isValidJSONObject(wrapped),
              let data = try? JSONSerialization.data(withJSONObject: wrapped, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return String(describing: value)
        }
        return text
    }

    private func dbToolName(forPath path: String) -> String? {
        let normalizedPath = path.hasPrefix("/control/db/") ? String(path.dropFirst("/control".count)) : path
        switch normalizedPath {
        case "/db/snapshot":
            return "db.snapshot"
        case "/db/app-storage":
            return "db.query_app_storage"
        case "/db/app-versions":
            return "db.query_app_versions"
        case "/db/bridge-calls":
            return "db.query_bridge_calls"
        case "/db/core-events":
            return "db.query_core_events"
        case "/db/test-runs":
            return "db.query_test_runs"
        case "/db/export-debug-bundle":
            return "db.export_debug_bundle"
        default:
            return nil
        }
    }

    private func dispatchDbTool(_ tool: String, args: [String: Any], sessionId: String) throws -> [String: Any] {
        switch tool {
        case "db.snapshot":
            return successBody(result: dbSnapshot(), sessionId: sessionId)
        case "db.query_app_storage",
             "db.query_app_versions",
             "db.query_bridge_calls",
             "db.query_core_events",
             "db.query_test_runs":
            return successBody(result: try dbQueryRows(tool: tool, args: args), sessionId: sessionId)
        case "db.export_debug_bundle":
            return successBody(result: try dbExportDebugBundle(), sessionId: sessionId)
        default:
            throw CommandError(
                status: 404,
                code: "platform_unsupported",
                message: "iOS dev control DB tool is not implemented",
                details: ["tool": tool]
            )
        }
    }

    private func dbSnapshot() -> [String: Any] {
        var tables: [String: Any] = [:]
        for spec in Self.dbSnapshotTables {
            tables[spec.table] = safeTableRows(spec, appId: nil, limit: 200)
        }
        return [
            "target": "ios-simulator",
            "platform": "ios",
            "tables": tables
        ]
    }

    private func dbQueryRows(tool: String, args: [String: Any]) throws -> [String: Any] {
        guard let spec = Self.safeDbTableByTool[tool] else {
            throw CommandError(status: 404, code: "platform_unsupported", message: "Unknown safe DB query", details: ["tool": tool])
        }
        let appId = args["appId"] as? String
        if spec.requiresAppId && (appId?.isEmpty ?? true) {
            throw CommandError(status: 400, code: "invalid_request", message: "\(tool) requires appId")
        }
        let rows = safeTableRows(spec, appId: appId, limit: limit(from: args))
        return [
            "table": spec.table,
            "columns": spec.columns,
            "appId": appId ?? NSNull(),
            "rows": rows
        ]
    }

    private func dbExportDebugBundle() throws -> [String: Any] {
        let exportId = "export_ios_\(UUID().uuidString.lowercased())"
        let createdAt = Self.now()
        let documentWithoutHash: [String: Any] = [
            "exportId": exportId,
            "type": "debug-bundle",
            "createdAt": createdAt,
            "runtimeVersion": "0.4.0",
            "source": [
                "platform": "ios",
                "target": "ios-simulator"
            ],
            "snapshot": dbSnapshot()
        ]
        let contentHashPrefix = "sha256:"
        let contentHash = "\(contentHashPrefix)\(Self.sha256Hex(jsonString(documentWithoutHash)))"
        var document = documentWithoutHash
        document["contentHash"] = contentHash
        let exportJson = jsonString(document)

        guard let db = database.handle else {
            throw CommandError(status: 500, code: "storage_error", message: "Platform database is not available")
        }
        let sql = """
        INSERT OR REPLACE INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at)
        VALUES (?, 'debug-bundle', 'ios', '0.4.0', ?, ?, ?)
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not prepare debug bundle export")
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, exportId)
        bind(statement, 2, exportJson)
        bind(statement, 3, contentHash)
        bind(statement, 4, createdAt)
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw CommandError(status: 500, code: "storage_error", message: "Could not persist debug bundle export")
        }
        return document
    }

    private func safeTableRows(_ spec: SafeDbTable, appId: String?, limit: Int) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let selectedColumns = spec.columns.joined(separator: ", ")
        let boundedLimit = max(1, min(limit, 500))
        let shouldFilter = appId?.isEmpty == false && spec.appFilterColumn != nil
        let whereClause = shouldFilter ? " WHERE \(spec.appFilterColumn!) = ?" : ""
        let orderClause = spec.orderBy.map { " ORDER BY \($0) DESC" } ?? ""
        let sql = "SELECT \(selectedColumns) FROM \(spec.table)\(whereClause)\(orderClause) LIMIT ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(statement) }
        if shouldFilter, let appId {
            bind(statement, 1, appId)
            sqlite3_bind_int64(statement, 2, Int64(boundedLimit))
        } else {
            sqlite3_bind_int64(statement, 1, Int64(boundedLimit))
        }

        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            var row: [String: Any] = [:]
            for (index, column) in spec.columns.enumerated() {
                row[column] = columnValue(statement, Int32(index))
            }
            rows.append(row)
        }
        return rows
    }

    private func limit(from args: [String: Any]) -> Int {
        if let limit = args["limit"] as? Int {
            return limit
        }
        if let number = args["limit"] as? NSNumber {
            return number.intValue
        }
        return 100
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

    private func columnValue(_ statement: OpaquePointer?, _ index: Int32) -> Any {
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

    private func jsonString(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return "{}"
        }
        return text
    }

    private func jsonFragmentString(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed, .sortedKeys]),
              let text = String(data: data, encoding: .utf8)
        else {
            return "null"
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

    private struct SafeDbTable {
        let table: String
        let columns: [String]
        let orderBy: String?
        let appFilterColumn: String?
        let requiresAppId: Bool
    }

    private struct ActiveAppMetadata {
        let activeInstallId: String?
        let activeVersion: String?
        let dataVersion: Int
    }

    private static let snapshotTypes: Set<String> = [
        "bug-report",
        "pre-install",
        "pre-migration",
        "post-test",
        "golden",
        "manual",
        "debug-bundle"
    ]

    private static let safeDbApps = SafeDbTable(
        table: "apps",
        columns: ["id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"],
        orderBy: "updated_at",
        appFilterColumn: "id",
        requiresAppId: false
    )
    private static let safeDbAppVersions = SafeDbTable(
        table: "app_versions",
        columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "manifest_hash", "content_hash", "trust_level", "status", "created_at", "activated_at"],
        orderBy: "created_at",
        appFilterColumn: "app_id",
        requiresAppId: true
    )
    private static let safeDbAppStorage = SafeDbTable(
        table: "app_storage",
        columns: ["app_id", "key", "value_json", "updated_at"],
        orderBy: "updated_at",
        appFilterColumn: "app_id",
        requiresAppId: true
    )
    private static let safeDbRuntimeSessions = SafeDbTable(
        table: "runtime_sessions",
        columns: ["session_id", "target", "platform", "runtime_version", "active_app_id", "status", "started_at", "ended_at", "capabilities_json", "resource_high_water_json", "metadata_json"],
        orderBy: "started_at",
        appFilterColumn: "active_app_id",
        requiresAppId: false
    )
    private static let safeDbBridgeCalls = SafeDbTable(
        table: "bridge_calls",
        columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"],
        orderBy: "created_at",
        appFilterColumn: "app_id",
        requiresAppId: false
    )
    private static let safeDbCoreEvents = SafeDbTable(
        table: "core_events",
        columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
        orderBy: "created_at",
        appFilterColumn: "app_id",
        requiresAppId: false
    )
    private static let safeDbCoreActions = SafeDbTable(
        table: "core_actions",
        columns: ["action_id", "event_id", "session_id", "app_id", "action_json", "created_at"],
        orderBy: "created_at",
        appFilterColumn: "app_id",
        requiresAppId: false
    )
    private static let safeDbRuntimeSnapshots = SafeDbTable(
        table: "runtime_snapshots",
        columns: ["snapshot_id", "session_id", "app_id", "install_id", "type", "content_hash", "created_at"],
        orderBy: "created_at",
        appFilterColumn: "app_id",
        requiresAppId: false
    )
    private static let safeDbControlSessions = SafeDbTable(
        table: "control_sessions",
        columns: ["control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"],
        orderBy: "started_at",
        appFilterColumn: nil,
        requiresAppId: false
    )
    private static let safeDbControlCommands = SafeDbTable(
        table: "control_commands",
        columns: ["command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "args_json", "result_json", "error_json", "created_at", "duration_ms"],
        orderBy: "created_at",
        appFilterColumn: nil,
        requiresAppId: false
    )
    private static let safeDbTestRuns = SafeDbTable(
        table: "test_runs",
        columns: ["test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at", "result_json", "diagnostics_json"],
        orderBy: "started_at",
        appFilterColumn: "app_id",
        requiresAppId: false
    )
    private static let safeDbBackupExports = SafeDbTable(
        table: "backup_exports",
        columns: ["export_id", "type", "source_platform", "runtime_version", "content_hash", "created_at", "imported_at"],
        orderBy: "created_at",
        appFilterColumn: nil,
        requiresAppId: false
    )

    private static let dbSnapshotTables = [
        safeDbApps,
        safeDbAppVersions,
        safeDbAppStorage,
        safeDbRuntimeSessions,
        safeDbBridgeCalls,
        safeDbCoreEvents,
        safeDbCoreActions,
        safeDbRuntimeSnapshots,
        safeDbControlSessions,
        safeDbControlCommands,
        safeDbTestRuns,
        safeDbBackupExports
    ]

    private static let safeDbTableByTool = [
        "db.query_app_storage": safeDbAppStorage,
        "db.query_app_versions": safeDbAppVersions,
        "db.query_bridge_calls": safeDbBridgeCalls,
        "db.query_core_events": safeDbCoreEvents,
        "db.query_test_runs": safeDbTestRuns
    ]

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
