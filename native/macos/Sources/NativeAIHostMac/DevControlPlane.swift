#if DEBUG
import Foundation
import Network
import Security
import SQLite3
import CryptoKit

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

    private struct InjectedFault {
        let code: String
        let message: String
        let details: [String: Any]
    }

    let token: String
    let tokenFileURL: URL
    let controlSessionId: String

    private let database: PlatformDatabase
    private let databaseURL: URL?
    private let core = ZigCoreBridge()
    private let queue = DispatchQueue(label: "dev.nativeai.macos.control-plane")
    private var listener: NWListener?
    private var sessionStatus = "running"
    private var activeRuntimeSessionId: String?
    private var activeAppId: String?
    private static let snapshotTypes: Set<String> = [
        "bug-report",
        "pre-install",
        "pre-migration",
        "post-test",
        "golden",
        "manual",
        "debug-bundle",
    ]

    var boundPort: UInt16? {
        listener?.port?.rawValue
    }

    init(configuration: Configuration = .defaultConfiguration()) throws {
        self.token = try configuration.tokenOverride ?? Self.generateToken()
        self.tokenFileURL = configuration.tokenFileURL
        self.controlSessionId = "control_\(UUID().uuidString.lowercased())"
        self.databaseURL = configuration.databaseURL
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
        case ("POST", "/db/app-storage"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbQueryAppStorage(args: parsed.jsonBody ?? [:]))
        case ("POST", "/db/app-versions"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbQueryAppVersions(args: parsed.jsonBody ?? [:]))
        case ("POST", "/db/bridge-calls"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbQueryBridgeCalls(args: parsed.jsonBody ?? [:]))
        case ("POST", "/db/core-events"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbQueryCoreEvents(args: parsed.jsonBody ?? [:]))
        case ("POST", "/db/test-runs"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: dbQueryTestRuns(args: parsed.jsonBody ?? [:]))
        case ("POST", "/db/export-debug-bundle"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: exportDebugBundle())
        case ("GET", "/apps"):
            sendAccepted(connection, parsed, startedAt: startedAt, result: listWebapps(includeUninstalled: false))
        default:
            if handleAppRoute(connection, parsed, startedAt: startedAt) {
                return
            }
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
            activeRuntimeSessionId = nil
            activeAppId = nil
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
        case ("GET", "capabilities"):
            sendAccepted(connection, request, startedAt: startedAt, result: runtimeCapabilities(appId: nil))
        case ("GET", "resource-usage"):
            sendAccepted(connection, request, startedAt: startedAt, result: resourceUsage(appId: nil))
        case ("GET", "accessibility"):
            sendAccepted(connection, request, startedAt: startedAt, result: accessibilityAudit(appId: nil))
        case ("POST", "snapshots") where route.itemId == nil:
            handleCreateSnapshot(connection, request, args: request.jsonBody ?? [:], startedAt: startedAt)
        case ("POST", "snapshots") where route.itemId != nil:
            if (request.jsonBody?["action"] as? String) == "restore" {
                handleRestoreSnapshot(connection, request, snapshotId: route.itemId ?? "", startedAt: startedAt)
            } else {
                handleReadSnapshot(connection, request, snapshotId: route.itemId ?? "", startedAt: startedAt)
            }
        case ("POST", "command"):
            handleCommand(connection, request, startedAt: startedAt)
        default:
            sendRejected(connection, request, status: 404, code: "not_found", message: "Control session route was not found", startedAt: startedAt)
        }
    }

    @discardableResult
    private func handleAppRoute(_ connection: NWConnection, _ request: HTTPRequest, startedAt: Date) -> Bool {
        guard let route = AppRoute(request.normalizedPath) else {
            return false
        }
        switch (request.method, route.subresource) {
        case ("GET", "versions"):
            handleListWebappVersions(connection, request, appId: route.appId, startedAt: startedAt)
        case ("POST", "rollback"):
            var args = request.jsonBody ?? [:]
            args["appId"] = route.appId
            handleRollbackWebapp(connection, request, args: args, startedAt: startedAt)
        case ("GET", "install-report"):
            handleInstallReport(connection, request, appId: route.appId, installId: nil, startedAt: startedAt)
        default:
            sendRejected(connection, request, status: 404, code: "not_found", message: "App control endpoint was not found", startedAt: startedAt)
        }
        return true
    }

    private func handleCommand(_ connection: NWConnection, _ request: HTTPRequest, startedAt: Date) {
        guard let body = request.jsonBody else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "Control command body must be JSON", startedAt: startedAt)
            return
        }
        let tool = body["tool"] as? String ?? ""
        let args = body["args"] as? [String: Any] ?? [:]
        switch tool {
        case "platform.health":
            sendAccepted(connection, request, startedAt: startedAt, result: healthResult())
        case "platform.list_targets":
            sendAccepted(connection, request, startedAt: startedAt, result: listTargets())
        case "platform.launch":
            sessionStatus = "running"
            sendAccepted(connection, request, startedAt: startedAt, result: sessionResult())
        case "platform.stop":
            sessionStatus = "ended"
            activeRuntimeSessionId = nil
            activeAppId = nil
            markControlSessionEnded()
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "ok": true,
                "target": "macos",
                "status": "stopped",
                "controlSessionId": controlSessionId,
            ])
        case "platform.reload_runtime":
            sendAccepted(connection, request, startedAt: startedAt, result: reloadRuntime())
        case "runtime.snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: snapshotResult())
        case "runtime.event_log":
            sendAccepted(connection, request, startedAt: startedAt, result: eventsResult())
        case "runtime.capabilities":
            sendAccepted(connection, request, startedAt: startedAt, result: runtimeCapabilities(appId: args["appId"] as? String))
        case "runtime.resource_usage":
            sendAccepted(connection, request, startedAt: startedAt, result: resourceUsage(appId: args["appId"] as? String))
        case "runtime.screenshot":
            handleRuntimeScreenshot(connection, request, args: args, startedAt: startedAt)
        case "runtime.query":
            handleRuntimeQuery(connection, request, args: args, startedAt: startedAt)
        case "runtime.click", "runtime.type", "runtime.set_value", "runtime.press_key", "runtime.drag":
            handleRuntimeTargetCommand(connection, request, args: args, startedAt: startedAt)
        case "runtime.wait_for":
            handleRuntimeWaitFor(connection, request, args: args, startedAt: startedAt)
        case "runtime.timer_advance":
            handleRuntimeTimerAdvance(connection, request, args: args, startedAt: startedAt)
        case "runtime.fault_inject":
            handleRuntimeFaultInject(connection, request, args: args, startedAt: startedAt)
        case "runtime.storage_get":
            handleRuntimeStorageGet(connection, request, args: args, startedAt: startedAt)
        case "runtime.storage_set":
            handleRuntimeStorageSet(connection, request, args: args, startedAt: startedAt)
        case "runtime.storage_reset", "platform.reset_webapp":
            handleRuntimeStorageReset(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_storage":
            handleStorageAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.network_mock_set":
            handleNetworkMockSet(connection, request, args: args, startedAt: startedAt)
        case "runtime.network_mock_reset":
            handleNetworkMockReset(connection, request, args: args, startedAt: startedAt)
        case "runtime.dialog_mock_set":
            handleDialogMockSet(connection, request, args: args, startedAt: startedAt)
        case "runtime.bridge_calls":
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "bridgeCalls": bridgeCallRows(appId: args["appId"] as? String),
            ])
        case "runtime.clear_logs":
            sendAccepted(connection, request, startedAt: startedAt, result: clearRuntimeLogs(appId: args["appId"] as? String))
        case "runtime.console_logs":
            sendAccepted(connection, request, startedAt: startedAt, result: consoleLogs(appId: args["appId"] as? String))
        case "runtime.notification_capture":
            sendAccepted(connection, request, startedAt: startedAt, result: notificationCapture(appId: args["appId"] as? String))
        case "runtime.assert_visible":
            handleVisibleAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_text":
            handleTextAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_bridge_call":
            handleBridgeCallAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_no_console_errors":
            handleNoConsoleErrorsAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.call_bridge":
            handleRuntimeCallBridge(connection, request, args: args, startedAt: startedAt)
        case "runtime.core_step":
            handleRuntimeCoreStep(connection, request, args: args, startedAt: startedAt)
        case "runtime.core_snapshot":
            handleRuntimeCoreSnapshot(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_core_action":
            handleCoreActionAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.accessibility_snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: accessibilitySnapshot(appId: args["appId"] as? String))
        case "runtime.run_accessibility_audit":
            sendAccepted(connection, request, startedAt: startedAt, result: accessibilityAudit(appId: args["appId"] as? String))
        case "runtime.assert_accessibility":
            handleAccessibilityAssertion(connection, request, args: args, startedAt: startedAt)
        case "platform.list_webapps":
            sendAccepted(connection, request, startedAt: startedAt, result: listWebapps(includeUninstalled: args["includeUninstalled"] as? Bool == true))
        case "platform.open_webapp":
            handleOpenWebapp(connection, request, args: args, startedAt: startedAt)
        case "platform.list_webapp_versions":
            guard let appId = args["appId"] as? String, !appId.isEmpty else {
                sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.list_webapp_versions requires appId", startedAt: startedAt)
                return
            }
            handleListWebappVersions(connection, request, appId: appId, startedAt: startedAt)
        case "platform.rollback_webapp":
            handleRollbackWebapp(connection, request, args: args, startedAt: startedAt)
        case "platform.install_report":
            guard let appId = args["appId"] as? String, !appId.isEmpty else {
                sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.install_report requires appId", startedAt: startedAt)
                return
            }
            handleInstallReport(connection, request, appId: appId, installId: args["installId"] as? String, startedAt: startedAt)
        case "platform.create_snapshot":
            handleCreateSnapshot(connection, request, args: args, startedAt: startedAt)
        case "platform.restore_snapshot":
            guard let snapshotId = args["snapshotId"] as? String, !snapshotId.isEmpty else {
                sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.restore_snapshot requires snapshotId", startedAt: startedAt)
                return
            }
            handleRestoreSnapshot(connection, request, snapshotId: snapshotId, startedAt: startedAt)
        case "db.snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: dbSnapshotResult())
        case "db.query_app_storage":
            sendAccepted(connection, request, startedAt: startedAt, result: dbQueryAppStorage(args: args))
        case "db.query_app_versions":
            sendAccepted(connection, request, startedAt: startedAt, result: dbQueryAppVersions(args: args))
        case "db.query_bridge_calls":
            sendAccepted(connection, request, startedAt: startedAt, result: dbQueryBridgeCalls(args: args))
        case "db.query_core_events":
            sendAccepted(connection, request, startedAt: startedAt, result: dbQueryCoreEvents(args: args))
        case "db.query_test_runs":
            sendAccepted(connection, request, startedAt: startedAt, result: dbQueryTestRuns(args: args))
        case "db.export_debug_bundle":
            sendAccepted(connection, request, startedAt: startedAt, result: exportDebugBundle())
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

    private func listTargets() -> [String: Any] {
        [
            "targets": [
                [
                    "id": "macos",
                    "platform": "macos",
                    "status": sessionStatus == "running" ? "available" : "stopped",
                    "runtimeVersion": "0.1.0",
                    "controlSessionId": controlSessionId,
                ],
                [
                    "id": "server",
                    "platform": "server",
                    "status": "not-attached",
                ],
                [
                    "id": "fake-host",
                    "platform": "fake-host",
                    "status": "not-attached",
                ],
            ],
        ]
    }

    private func sessionResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "runtimeSessionId": activeRuntimeSessionId.map { $0 as Any } ?? NSNull(),
            "target": "macos",
            "appId": activeAppId.map { $0 as Any } ?? NSNull(),
            "status": sessionStatus,
        ]
    }

    private func snapshotResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "snapshot": [
                "platform": "macos",
                "target": "macos",
                "activeAppId": activeAppId.map { $0 as Any } ?? NSNull(),
                "runtimeSessionId": activeRuntimeSessionId.map { $0 as Any } ?? NSNull(),
                "runtimeAttached": activeRuntimeSessionId != nil,
                "controlCommands": controlCommandCount(),
            ],
        ]
    }

    private func eventsResult() -> [String: Any] {
        [
            "controlSessionId": controlSessionId,
            "runtimeSessionId": activeRuntimeSessionId.map { $0 as Any } ?? NSNull(),
            "appId": activeAppId.map { $0 as Any } ?? NSNull(),
            "bridgeCalls": bridgeCallRows(appId: activeAppId),
            "coreEvents": tableRows(
                table: "core_events",
                columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
                orderBy: "created_at",
                filterColumn: "app_id",
                filterValue: activeAppId
            ),
            "controlCommands": controlCommands(),
        ]
    }

    private func runtimeCapabilities(appId: String?) -> [String: Any] {
        [
            "runtimeVersion": "0.1.0",
            "platform": "macos",
            "target": "macos",
            "appId": appId ?? NSNull(),
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
                "core.step": core.isAvailable,
                "runtime.capabilities": true,
                "app.log": true,
            ],
            "limits": [
                "maxPackageBytes": 1_048_576,
                "maxFileBytes": 524_288,
            ],
        ]
    }

    private func resourceUsage(appId: String?) -> [String: Any] {
        let since = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-60))
        let storageSQL = appId == nil
            ? "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage"
            : "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?"
        let bridgeSQL = appId == nil
            ? "SELECT COUNT(*) FROM bridge_calls"
            : "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?"
        let coreSQL = appId == nil
            ? "SELECT COUNT(*) FROM core_events"
            : "SELECT COUNT(*) FROM core_events WHERE app_id = ?"
        let packageSQL = appId == nil
            ? "SELECT COALESCE(SUM(size_bytes), 0) FROM app_files"
            : """
            SELECT COALESCE(SUM(f.size_bytes), 0)
            FROM app_files f
            JOIN app_versions v ON v.install_id = f.install_id
            WHERE v.app_id = ?
            """
        let networkSQL = appId == nil
            ? "SELECT COUNT(*) FROM bridge_calls WHERE method = 'network.request' AND created_at >= ?"
            : "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'network.request' AND created_at >= ?"
        let logSQL = appId == nil
            ? "SELECT COUNT(*) FROM bridge_calls WHERE method = 'app.log' AND created_at >= ?"
            : "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'app.log' AND created_at >= ?"
        let appValues = appId.map { [$0] } ?? []

        return [
            "appId": appId ?? NSNull(),
            "storageBytes": scalarInt(storageSQL, values: appValues),
            "bridgeCalls": scalarInt(bridgeSQL, values: appValues),
            "coreEvents": scalarInt(coreSQL, values: appValues),
            "networkRequestsLastMinute": scalarInt(networkSQL, values: appId.map { [$0, since] } ?? [since]),
            "logLinesLastMinute": scalarInt(logSQL, values: appId.map { [$0, since] } ?? [since]),
            "domNodes": 0,
            "timers": 0,
            "packageBytes": scalarInt(packageSQL, values: appValues),
        ]
    }

    private func accessibilitySnapshot(appId: String?) -> [String: Any] {
        let actualAppId = appId ?? "notes-lite"
        let html = htmlForBundledApp(actualAppId)
        return [
            "appId": actualAppId,
            "title": firstMatch(in: html, pattern: #"<title[^>]*>([\s\S]*?)</title>"#),
            "landmarks": html.range(of: #"<main\b"#, options: [.regularExpression, .caseInsensitive]) == nil ? [] : [
                ["role": "main", "selector": "main"],
            ],
            "headings": headingRecords(html),
            "controls": controlRecords(html),
        ]
    }

    private func accessibilityAudit(appId: String?) -> [String: Any] {
        let actualAppId = appId ?? "notes-lite"
        let snapshot = accessibilitySnapshot(appId: actualAppId)
        let title = snapshot["title"] as? String ?? ""
        let landmarks = snapshot["landmarks"] as? [[String: Any]] ?? []
        let headings = snapshot["headings"] as? [[String: Any]] ?? []
        let controls = snapshot["controls"] as? [[String: Any]] ?? []
        let unlabeled = controls.first { ($0["name"] as? String ?? "").isEmpty }
        let checks: [[String: Any]] = [
            accessibilityCheck(id: "document_title", ok: !title.isEmpty, message: "Document must include a non-empty <title>."),
            accessibilityCheck(id: "main_landmark", ok: landmarks.contains { $0["role"] as? String == "main" }, message: "Page must include a <main> landmark."),
            accessibilityCheck(id: "screen_title", ok: headings.contains { $0["level"] as? Int == 1 }, message: "Page must include an h1 screen title."),
            accessibilityCheck(
                id: "no_unlabeled_controls",
                ok: unlabeled == nil,
                message: "Every interactive control must have an accessible name.",
                selector: unlabeled?["selector"] as? String
            ),
        ]
        let status = checks.contains { $0["status"] as? String == "fail" } ? "fail" : "pass"
        return [
            "appId": actualAppId,
            "checkedAt": Self.now(),
            "status": status,
            "checks": checks,
        ]
    }

    private func handleAccessibilityAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let report = accessibilityAudit(appId: args["appId"] as? String)
        let rule = args["rule"] as? String
        let checks = report["checks"] as? [[String: Any]] ?? []
        let failures = checks.filter { check in
            (check["status"] as? String) == "fail" && (rule == nil || (check["id"] as? String) == rule)
        }
        guard failures.isEmpty else {
            audit(request, decision: "rejected", errorCode: "accessibility_failed", startedAt: startedAt, result: nil)
            send(connection, status: 400, body: errorBody("accessibility_failed", "Accessibility assertion failed", sessionId: controlSessionId))
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": report["appId"] ?? NSNull(),
            "rule": rule ?? NSNull(),
            "report": report,
        ])
    }

    private func handleListWebappVersions(_ connection: NWConnection, _ request: HTTPRequest, appId: String, startedAt: Date) {
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "appId": appId,
            "versions": listWebappVersions(appId: appId),
        ])
    }

    private func handleOpenWebapp(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.open_webapp requires appId", startedAt: startedAt)
            return
        }
        guard activeAppRecord(appId: appId)?.installId != nil else {
            sendRejected(connection, request, status: 400, code: "app_not_installed", message: "App is not installed", startedAt: startedAt)
            return
        }
        let sessionId = runtimeSessionId(appId: appId)
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "sessionId": sessionId,
            "runtimeSessionId": sessionId,
            "appId": appId,
            "target": "macos",
        ])
    }

    private func handleRuntimeQuery(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.query requires appId", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: runtimeQuery(appId: appId, args: args))
    }

    private func handleRuntimeScreenshot(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.screenshot requires appId", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: runtimeScreenshot(appId: appId, label: args["label"] as? String))
    }

    private func handleRuntimeTargetCommand(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        if request.toolName == "runtime.press_key" {
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "ok": true,
                "key": args["key"] ?? NSNull(),
            ])
            return
        }
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "\(request.toolName) requires appId", startedAt: startedAt)
            return
        }
        let result = runtimeQuery(appId: appId, args: args)
        let matches = result["matches"] as? [[String: Any]] ?? []
        guard let target = matches.first else {
            sendRejected(connection, request, status: 400, code: "selector.not_found", message: "Runtime target was not found in generated app HTML", startedAt: startedAt)
            return
        }
        var response: [String: Any] = [
            "ok": true,
            "tool": request.toolName,
            "target": target,
        ]
        if request.toolName == "runtime.type" || request.toolName == "runtime.set_value" {
            response["value"] = args["value"] ?? args["text"] ?? ""
        }
        sendAccepted(connection, request, startedAt: startedAt, result: response)
    }

    private func handleRuntimeWaitFor(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let kind = args["kind"] as? String ?? "idle"
        if kind == "idle" {
            sendAccepted(connection, request, startedAt: startedAt, result: ["ok": true, "kind": kind])
            return
        }
        if kind == "bridge_call" || kind == "bridgeCall" {
            guard let appId = args["appId"] as? String, !appId.isEmpty,
                  let method = args["method"] as? String, !method.isEmpty
            else {
                sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.wait_for bridge_call requires appId and method", startedAt: startedAt)
                return
            }
            let rows = bridgeCallRows(appId: appId).filter { ($0["method"] as? String) == method }
            guard let latest = rows.last else {
                sendRejected(connection, request, status: 400, code: "wait_timeout", message: "Expected bridge call was not recorded", startedAt: startedAt)
                return
            }
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "ok": true,
                "kind": kind,
                "appId": appId,
                "method": method,
                "latest": latest,
            ])
            return
        }
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.wait_for requires appId for selector/text waits", startedAt: startedAt)
            return
        }
        let query = runtimeQuery(appId: appId, args: args)
        let matches = query["matches"] as? [[String: Any]] ?? []
        guard !matches.isEmpty else {
            sendRejected(connection, request, status: 400, code: "wait_timeout", message: "Expected runtime condition did not appear", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "kind": kind,
            "appId": appId,
            "matches": matches.count,
        ])
    }

    private func handleRuntimeTimerAdvance(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let milliseconds = intValue(args["ms"]) ?? intValue(args["milliseconds"]) ?? 0
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "advancedMs": max(0, milliseconds),
        ])
    }

    private func handleRuntimeFaultInject(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let method = args["method"] as? String ?? methodForFaultKind(args["kind"] as? String)
        guard let method, !method.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.fault_inject requires a bridge method", startedAt: startedAt)
            return
        }
        guard isKnownControlBridgeMethod(method) else {
            sendRejected(connection, request, status: 400, code: "unknown_method", message: "Unknown bridge method: \(method)", startedAt: startedAt)
            return
        }
        let code = args["code"] as? String ?? "fault_injected"
        let message = args["message"] as? String ?? "Injected bridge fault"
        let details = args["details"] ?? faultDetails(kind: args["kind"] as? String)
        let once = (args["once"] as? Bool) ?? true
        guard let result = addFaultInjection(
            sessionId: args["sessionId"] as? String,
            appId: args["appId"] as? String,
            method: method,
            code: code,
            message: message,
            details: details,
            once: once
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Fault injection could not be registered", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleVisibleAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.assert_visible requires appId", startedAt: startedAt)
            return
        }
        let result = runtimeQuery(appId: appId, args: args)
        let matches = result["matches"] as? [[String: Any]] ?? []
        guard let target = matches.first else {
            sendRejected(connection, request, status: 400, code: "selector.not_found", message: "Expected runtime target is not visible", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "matches": matches.count,
            "target": target,
        ])
    }

    private func handleTextAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let text = args["text"] as? String, !text.isEmpty
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.assert_text requires appId and text", startedAt: startedAt)
            return
        }
        guard htmlText(htmlForBundledApp(appId)).contains(text) else {
            sendRejected(connection, request, status: 400, code: "text.not_found", message: "Expected text was not found in installed package HTML", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "text": text,
        ])
    }

    private func handleRollbackWebapp(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.rollback_webapp requires appId", startedAt: startedAt)
            return
        }
        do {
            let registry = try PlatformAppRegistry(databaseURL: databaseURL)
            let result = try registry.rollback(
                appId: appId,
                targetInstallId: args["installId"] as? String,
                actor: "codex"
            )
            sendAccepted(connection, request, startedAt: startedAt, result: [
                "appId": result.appId,
                "activeInstallId": result.activeInstallId,
                "rolledBackInstallId": result.rolledBackInstallId,
                "activeVersion": result.activeVersion,
            ])
        } catch {
            sendRejected(connection, request, status: 400, code: "rollback_failed", message: "Rollback could not be completed", startedAt: startedAt)
        }
    }

    private func handleInstallReport(_ connection: NWConnection, _ request: HTTPRequest, appId: String, installId: String?, startedAt: Date) {
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "appId": appId,
            "installId": installId ?? NSNull(),
            "report": installReport(appId: appId, installId: installId) ?? NSNull(),
        ])
    }

    private func handleRuntimeStorageGet(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let key = args["key"] as? String, !key.isEmpty
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.storage_get requires appId and key", startedAt: startedAt)
            return
        }
        let response = PlatformStorage(databaseURL: databaseURL).get(BridgeRequest(
            id: args["id"] as? String ?? "control_storage_get",
            method: "storage.get",
            params: ["key": key, "defaultValue": args["defaultValue"] ?? NSNull()],
            context: controlStorageContext(appId: appId, permissions: ["storage.read"])
        ))
        recordBridgeCall(appId: appId, method: "storage.get", params: ["key": key], response: response, startedAt: startedAt)
        sendAccepted(connection, request, startedAt: startedAt, result: response.asDictionary())
    }

    private func handleRuntimeStorageSet(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let key = args["key"] as? String, !key.isEmpty
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.storage_set requires appId and key", startedAt: startedAt)
            return
        }
        let response = PlatformStorage(databaseURL: databaseURL).set(BridgeRequest(
            id: args["id"] as? String ?? "control_storage_set",
            method: "storage.set",
            params: ["key": key, "value": args["value"] ?? NSNull()],
            context: controlStorageContext(appId: appId, permissions: ["storage.write"])
        ))
        recordBridgeCall(appId: appId, method: "storage.set", params: ["key": key, "value": args["value"] ?? NSNull()], response: response, startedAt: startedAt)
        sendAccepted(connection, request, startedAt: startedAt, result: response.asDictionary())
    }

    private func handleRuntimeStorageReset(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "\(request.toolName) requires appId", startedAt: startedAt)
            return
        }
        guard let result = resetWebapp(appId: appId) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Webapp storage could not be reset", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleStorageAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let key = args["key"] as? String, !key.isEmpty,
              args.keys.contains("value")
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.assert_storage requires appId, key, and value", startedAt: startedAt)
            return
        }
        let expected = args["value"] ?? NSNull()
        guard let actual = storageValue(appId: appId, key: key) else {
            sendRejected(connection, request, status: 400, code: "assertion_failed", message: "Expected storage key was not found", startedAt: startedAt)
            return
        }
        guard canonicalJSONEqual(actual, expected) else {
            sendRejected(connection, request, status: 400, code: "assertion_failed", message: "Storage value did not match expected value", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "key": key,
            "value": actual,
        ])
    }

    private func handleNetworkMockSet(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let match = args["match"] as? [String: Any] ?? [:]
        guard let urlPattern = args["urlPattern"] as? String ?? match["urlPattern"] as? String ?? match["url"] as? String,
              !urlPattern.isEmpty,
              args.keys.contains("response")
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.network_mock_set requires urlPattern or match.url and response", startedAt: startedAt)
            return
        }
        let method = (args["method"] as? String ?? match["method"] as? String ?? "GET").uppercased()
        guard let result = addNetworkMock(
            sessionId: args["sessionId"] as? String,
            appId: args["appId"] as? String,
            method: method,
            urlPattern: urlPattern,
            response: args["response"] ?? NSNull()
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Network mock could not be registered", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleNetworkMockReset(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let sessionId = args["sessionId"] as? String
        let appId = args["appId"] as? String
        let result: [String: Any]
        if let sessionId, let appId {
            result = ["ok": true, "cleared": deleteRows("DELETE FROM network_mocks WHERE session_id = ? AND app_id = ?", values: [sessionId, appId])]
        } else if let sessionId {
            result = ["ok": true, "cleared": deleteRows("DELETE FROM network_mocks WHERE session_id = ?", values: [sessionId])]
        } else if let appId {
            result = ["ok": true, "cleared": deleteRows("DELETE FROM network_mocks WHERE app_id = ?", values: [appId])]
        } else {
            result = ["ok": true, "cleared": deleteRows("DELETE FROM network_mocks")]
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleDialogMockSet(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let dialogType = normalizedDialogType(args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.dialog_mock_set requires dialogType or method", startedAt: startedAt)
            return
        }
        let response = args["response"] ?? [
            "files": args["files"] ?? [],
            "selectedPath": args["selectedPath"] ?? NSNull(),
            "cancelled": args["cancelled"] ?? false,
        ]
        guard let result = addDialogMock(
            sessionId: args["sessionId"] as? String,
            appId: args["appId"] as? String,
            dialogType: dialogType,
            response: response
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Dialog mock could not be registered", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleBridgeCallAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let method = args["method"] as? String, !method.isEmpty
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.assert_bridge_call requires appId and method", startedAt: startedAt)
            return
        }
        let rows = bridgeCallRows(appId: appId).filter { ($0["method"] as? String) == method }
        guard let latest = rows.last else {
            sendRejected(connection, request, status: 400, code: "assertion_failed", message: "Expected bridge call was not recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "method": method,
            "count": rows.count,
            "latest": latest,
        ])
    }

    private func handleNoConsoleErrorsAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        let errors = consoleLogRows(appId: args["appId"] as? String).filter { row in
            let params = jsonDictionary(row["params_json"] as? String ?? "") ?? [:]
            return (params["level"] as? String) == "error"
        }
        guard errors.isEmpty else {
            sendRejected(connection, request, status: 400, code: "assertion_failed", message: "Console errors were recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "errors": 0,
        ])
    }

    private func handleRuntimeCallBridge(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let method = args["method"] as? String, !method.isEmpty
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.call_bridge requires appId and method", startedAt: startedAt)
            return
        }
        let params = args["params"] as? [String: Any] ?? [:]
        let response = dispatchControlBridge(
            appId: appId,
            method: method,
            params: params,
            requestId: args["id"] as? String ?? "control_call_bridge",
            startedAt: startedAt
        )
        sendAccepted(connection, request, startedAt: startedAt, result: response.asDictionary())
    }

    private func handleRuntimeCoreStep(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty,
              let event = args["event"]
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.core_step requires appId and event", startedAt: startedAt)
            return
        }
        let response = dispatchControlBridge(
            appId: appId,
            method: "core.step",
            params: ["event": event],
            requestId: args["id"] as? String ?? "control_core_step",
            startedAt: startedAt
        )
        sendAccepted(connection, request, startedAt: startedAt, result: response.asDictionary())
    }

    private func handleRuntimeCoreSnapshot(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String ?? activeAppId, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.core_snapshot requires appId or an active runtime app", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: coreSnapshot(appId: appId))
    }

    private func handleCoreActionAssertion(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.assert_core_action requires appId", startedAt: startedAt)
            return
        }
        let expectedType = args["type"] as? String ?? args["actionType"] as? String
        let expectedAction = args["action"]
        let rows = coreActionRows(appId: appId).filter { row in
            let action = jsonDictionary(row["action_json"] as? String ?? "") ?? [:]
            if let expectedType, (action["type"] as? String) != expectedType {
                return false
            }
            if let expectedAction, !canonicalJSONEqual(action, expectedAction) {
                return false
            }
            return true
        }
        guard let latest = rows.last else {
            sendRejected(connection, request, status: 400, code: "assertion_failed", message: "Expected core action was not recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "type": expectedType ?? NSNull(),
            "count": rows.count,
            "latest": latest,
            "action": jsonValue(latest["action_json"] as? String),
        ])
    }

    private func handleCreateSnapshot(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.create_snapshot requires appId", startedAt: startedAt)
            return
        }
        let type = args["type"] as? String ?? "manual"
        guard Self.snapshotTypes.contains(type) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "Snapshot type is not allowed", startedAt: startedAt)
            return
        }
        guard let result = createSnapshot(appId: appId, type: type, sessionId: args["sessionId"] as? String) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Snapshot could not be created", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleReadSnapshot(_ connection: NWConnection, _ request: HTTPRequest, snapshotId: String, startedAt: Date) {
        guard let snapshot = readSnapshot(snapshotId: snapshotId) else {
            sendRejected(connection, request, status: 404, code: "snapshot_not_found", message: "Snapshot was not found", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: snapshot)
    }

    private func handleRestoreSnapshot(_ connection: NWConnection, _ request: HTTPRequest, snapshotId: String, startedAt: Date) {
        guard let result = restoreSnapshot(snapshotId: snapshotId) else {
            sendRejected(connection, request, status: 404, code: "snapshot_not_found", message: "Snapshot was not found", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func createSnapshot(appId: String, type: String, sessionId: String?) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let active = activeAppRecord(appId: appId)
        let createdAt = Self.now()
        let storage = tableRows(
            table: "app_storage",
            columns: ["app_id", "key", "value_json", "updated_at"],
            orderBy: "key",
            filterColumn: "app_id",
            filterValue: appId
        )
        var snapshot: [String: Any] = [
            "appId": appId,
            "activeInstallId": active?.installId ?? NSNull(),
            "activeVersion": active?.version ?? NSNull(),
            "dataVersion": active?.dataVersion ?? NSNull(),
            "storage": storage,
            "createdAt": createdAt,
        ]
        let snapshotJSON = jsonBody(snapshot)
        let contentHash = "sha256:\(sha256Hex(snapshotJSON))"
        let snapshotId = "snapshot_\(UUID().uuidString.lowercased())"
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO runtime_snapshots (snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, snapshotId)
        bindNullable(statement, 2, sessionId)
        bind(statement, 3, appId)
        bindNullable(statement, 4, active?.installId)
        bind(statement, 5, type)
        bind(statement, 6, snapshotJSON)
        bind(statement, 7, contentHash)
        bind(statement, 8, createdAt)
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return nil
        }
        snapshot["snapshotId"] = snapshotId
        return snapshot
    }

    private func readSnapshot(snapshotId: String) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT snapshot_id, snapshot_json, content_hash, created_at FROM runtime_snapshots WHERE snapshot_id = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, snapshotId)
        guard sqlite3_step(statement) == SQLITE_ROW,
              let snapshot = jsonDictionary(columnText(statement, 1))
        else {
            return nil
        }
        return [
            "snapshotId": columnText(statement, 0),
            "snapshot": snapshot,
            "contentHash": columnText(statement, 2),
            "createdAt": columnText(statement, 3),
        ]
    }

    private func restoreSnapshot(snapshotId: String) -> [String: Any]? {
        guard let record = readSnapshot(snapshotId: snapshotId),
              let snapshot = record["snapshot"] as? [String: Any]
        else {
            return nil
        }
        let appId = snapshot["appId"] as? String
        let storage = snapshot["storage"] as? [[String: Any]] ?? []
        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true
        if let appId {
            ok = deleteStorage(appId: appId)
        }
        if ok {
            for item in storage {
                guard insertStorageRow(item, fallbackAppId: appId) else {
                    ok = false
                    break
                }
            }
        }
        if ok, let activeInstallId = snapshot["activeInstallId"] as? String, let appId {
            ok = updateActiveAppAfterRestore(
                appId: appId,
                activeInstallId: activeInstallId,
                activeVersion: snapshot["activeVersion"] as? String,
                dataVersion: intValue(snapshot["dataVersion"])
            )
        }
        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            return nil
        }
        return [
            "ok": true,
            "snapshotId": snapshotId,
            "appId": appId ?? NSNull(),
            "restoredStorageKeys": storage.count,
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
            "backup_exports": tableRows(
                table: "backup_exports",
                columns: ["export_id", "type", "source_platform", "runtime_version", "content_hash", "created_at", "imported_at"],
                orderBy: "created_at"
            ),
        ]
    }

    private func dbQueryAppStorage(args: [String: Any]) -> [String: Any] {
        [
            "rows": tableRows(
                table: "app_storage",
                columns: ["app_id", "key", "value_json", "updated_at"],
                orderBy: "updated_at",
                filterColumn: "app_id",
                filterValue: args["appId"] as? String
            ),
        ]
    }

    private func dbQueryAppVersions(args: [String: Any]) -> [String: Any] {
        [
            "rows": tableRows(
                table: "app_versions",
                columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "content_hash", "status", "created_at", "activated_at"],
                orderBy: "created_at",
                filterColumn: "app_id",
                filterValue: args["appId"] as? String
            ),
        ]
    }

    private func dbQueryBridgeCalls(args: [String: Any]) -> [String: Any] {
        [
            "rows": tableRows(
                table: "bridge_calls",
                columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"],
                orderBy: "created_at",
                filterColumn: "app_id",
                filterValue: args["appId"] as? String
            ),
        ]
    }

    private func dbQueryCoreEvents(args: [String: Any]) -> [String: Any] {
        [
            "rows": tableRows(
                table: "core_events",
                columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
                orderBy: "created_at",
                filterColumn: "app_id",
                filterValue: args["appId"] as? String
            ),
        ]
    }

    private func dbQueryTestRuns(args: [String: Any]) -> [String: Any] {
        [
            "rows": tableRows(
                table: "test_runs",
                columns: ["test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"],
                orderBy: "started_at",
                filterColumn: "app_id",
                filterValue: args["appId"] as? String
            ),
        ]
    }

    private func listWebapps(includeUninstalled: Bool) -> [String: Any] {
        guard let db = database.handle else { return ["apps": []] }
        let filterSQL = includeUninstalled ? "" : "WHERE a.status <> 'uninstalled'"
        let sql = """
        SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version, a.created_at, a.updated_at, v.runtime_version, v.trust_level
        FROM apps a
        LEFT JOIN app_versions v ON v.install_id = a.active_install_id
        \(filterSQL)
        ORDER BY a.id
        LIMIT 100
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return ["apps": []]
        }
        defer { sqlite3_finalize(statement) }
        var apps: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            apps.append([
                "appId": columnText(statement, 0),
                "name": columnText(statement, 1),
                "status": columnText(statement, 2),
                "activeInstallId": columnNullableText(statement, 3) ?? NSNull(),
                "activeVersion": columnNullableText(statement, 4) ?? NSNull(),
                "dataVersion": Int(sqlite3_column_int64(statement, 5)),
                "createdAt": columnText(statement, 6),
                "updatedAt": columnText(statement, 7),
                "runtimeVersion": columnNullableText(statement, 8) ?? NSNull(),
                "trustLevel": columnNullableText(statement, 9) ?? NSNull(),
            ])
        }
        return ["apps": apps]
    }

    private func listWebappVersions(appId: String) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let sql = """
        SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at
        FROM app_versions
        WHERE app_id = ?
        ORDER BY created_at DESC
        LIMIT 100
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        var versions: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            versions.append([
                "installId": columnText(statement, 0),
                "appId": columnText(statement, 1),
                "appVersion": columnText(statement, 2),
                "runtimeVersion": columnText(statement, 3),
                "dataVersion": Int(sqlite3_column_int64(statement, 4)),
                "manifestHash": columnText(statement, 5),
                "contentHash": columnText(statement, 6),
                "signature": jsonValue(columnNullableText(statement, 7)),
                "trustLevel": columnText(statement, 8),
                "status": columnText(statement, 9),
                "installedAt": columnText(statement, 10),
                "activatedAt": columnNullableText(statement, 11) ?? NSNull(),
            ])
        }
        return versions
    }

    private func reloadRuntime() -> [String: Any] {
        guard let appId = activeAppId else {
            return [
                "ok": true,
                "target": "macos",
                "status": "reloaded",
                "runtimeSessionId": NSNull(),
                "appId": NSNull(),
            ]
        }
        let sessionId = runtimeSessionId(appId: appId)
        return [
            "ok": true,
            "target": "macos",
            "status": "reloaded",
            "runtimeSessionId": sessionId,
            "appId": appId,
        ]
    }

    private func installReport(appId: String, installId: String?) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let sql = installId == nil
            ? """
            SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at
            FROM app_install_reports
            WHERE app_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            """
            : """
            SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at
            FROM app_install_reports
            WHERE app_id = ? AND install_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        if let installId {
            bind(statement, 2, installId)
        }
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return [
            "reportId": columnText(statement, 0),
            "appId": columnText(statement, 1),
            "installId": columnNullableText(statement, 2) ?? NSNull(),
            "status": columnText(statement, 3),
            "validation": jsonValue(columnNullableText(statement, 4)),
            "security": jsonValue(columnNullableText(statement, 5)),
            "permissions": jsonValue(columnNullableText(statement, 6)),
            "compatibility": jsonValue(columnNullableText(statement, 7)),
            "smokeTest": jsonValue(columnNullableText(statement, 8)),
            "contentHash": columnNullableText(statement, 9) ?? NSNull(),
            "createdAt": columnText(statement, 10),
        ]
    }

    private func resetWebapp(appId: String) -> [String: Any]? {
        let storageKeys = scalarInt("SELECT COUNT(*) FROM app_storage WHERE app_id = ?", values: [appId])
        let snapshot = createSnapshot(appId: appId, type: "manual", sessionId: runtimeSessionId(appId: appId))
        guard deleteStorage(appId: appId) else {
            return nil
        }
        return [
            "ok": true,
            "appId": appId,
            "snapshotId": snapshot?["snapshotId"] ?? NSNull(),
            "clearedStorageKeys": storageKeys,
        ]
    }

    private func storageValue(appId: String, key: String) -> Any? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return jsonValue(columnNullableText(statement, 0))
    }

    private func addNetworkMock(sessionId: String?, appId: String?, method: String, urlPattern: String, response: Any) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let mockId = "netmock_\(UUID().uuidString.lowercased())"
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at)
        VALUES (?, ?, ?, ?, ?, ?, 1, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, mockId)
        bindNullable(statement, 2, sessionId)
        bindNullable(statement, 3, appId)
        bind(statement, 4, method)
        bind(statement, 5, urlPattern)
        bind(statement, 6, jsonString(response))
        bind(statement, 7, Self.now())
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return nil
        }
        return [
            "ok": true,
            "mockId": mockId,
            "sessionId": sessionId ?? NSNull(),
            "appId": appId ?? NSNull(),
            "method": method,
            "urlPattern": urlPattern,
        ]
    }

    private func addDialogMock(sessionId: String?, appId: String?, dialogType: String, response: Any) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let mockId = "dialogmock_\(UUID().uuidString.lowercased())"
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at)
        VALUES (?, ?, ?, ?, ?, 1, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, mockId)
        bindNullable(statement, 2, sessionId)
        bindNullable(statement, 3, appId)
        bind(statement, 4, dialogType)
        bind(statement, 5, jsonString(response))
        bind(statement, 6, Self.now())
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return nil
        }
        return [
            "ok": true,
            "mockId": mockId,
            "sessionId": sessionId ?? NSNull(),
            "appId": appId ?? NSNull(),
            "dialogType": dialogType,
        ]
    }

    private func addFaultInjection(sessionId: String?, appId: String?, method: String, code: String, message: String, details: Any, once: Bool) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let faultId = "fault_\(UUID().uuidString.lowercased())"
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO fault_injections (fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, faultId)
        bindNullable(statement, 2, sessionId)
        bindNullable(statement, 3, appId)
        bind(statement, 4, method)
        bind(statement, 5, code)
        bind(statement, 6, message)
        bind(statement, 7, jsonString(details))
        sqlite3_bind_int64(statement, 8, once ? 1 : 0)
        bind(statement, 9, Self.now())
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return nil
        }
        return [
            "ok": true,
            "faultId": faultId,
            "appId": appId ?? NSNull(),
            "method": method,
            "code": code,
            "message": message,
            "details": details,
            "once": once,
        ]
    }

    private func findNetworkMock(sessionId: String?, appId: String, method: String, url: String) -> Any? {
        guard let db = database.handle else { return nil }
        let sql = """
        SELECT response_json, url_pattern FROM network_mocks
        WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)
        ORDER BY created_at DESC
        LIMIT 100
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, method.uppercased())
        bind(statement, 2, appId)
        bind(statement, 3, sessionId ?? "")
        while sqlite3_step(statement) == SQLITE_ROW {
            let pattern = columnText(statement, 1)
            if urlMatches(pattern: pattern, url: url) {
                return jsonValue(columnNullableText(statement, 0))
            }
        }
        return nil
    }

    private func findDialogMock(sessionId: String?, appId: String, dialogType: String) -> Any? {
        guard let db = database.handle else { return nil }
        let sql = """
        SELECT response_json FROM dialog_mocks
        WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)
        ORDER BY created_at DESC
        LIMIT 1
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, dialogType)
        bind(statement, 2, appId)
        bind(statement, 3, sessionId ?? "")
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return jsonValue(columnNullableText(statement, 0))
    }

    private func takeInjectedFault(appId: String, sessionId: String?, method: String) -> InjectedFault? {
        guard let db = database.handle else { return nil }
        let sql = """
        SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections
        WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?)
        ORDER BY created_at
        LIMIT 1
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, method)
        bind(statement, 2, appId)
        bind(statement, 3, sessionId ?? "")
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        let faultId = columnText(statement, 0)
        let code = columnText(statement, 1)
        let message = columnText(statement, 2)
        var details = jsonDictionary(columnText(statement, 3)) ?? [:]
        details["faultId"] = faultId
        details["appId"] = appId
        details["method"] = method
        if sqlite3_column_int64(statement, 4) != 0 {
            _ = deleteRows("UPDATE fault_injections SET enabled = 0 WHERE fault_id = ?", values: [faultId])
        }
        return InjectedFault(code: code, message: message, details: details)
    }

    private func bridgeCallRows(appId: String?) -> [[String: Any]] {
        tableRows(
            table: "bridge_calls",
            columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "params_json", "result_json", "error_json", "duration_ms", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId
        )
    }

    private func coreSnapshot(appId: String) -> [String: Any] {
        [
            "appId": appId,
            "stateVersion": scalarInt(
                "SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?",
                values: [appId]
            ),
            "coreEvents": coreEventRows(appId: appId).map { row in
                var event = row
                event["event"] = jsonValue(row["event_json"] as? String)
                return event
            },
            "coreActions": coreActionRows(appId: appId).map { row in
                var action = row
                action["action"] = jsonValue(row["action_json"] as? String)
                return action
            },
        ]
    }

    private func coreEventRows(appId: String?) -> [[String: Any]] {
        tableRows(
            table: "core_events",
            columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId
        )
    }

    private func coreActionRows(appId: String?) -> [[String: Any]] {
        tableRows(
            table: "core_actions",
            columns: ["action_id", "event_id", "session_id", "app_id", "action_json", "created_at"],
            orderBy: "created_at",
            filterColumn: "app_id",
            filterValue: appId
        )
    }

    private func consoleLogs(appId: String?) -> [String: Any] {
        [
            "appId": appId ?? NSNull(),
            "logs": consoleLogRows(appId: appId).map { row in
                [
                    "bridgeCallId": row["bridge_call_id"] ?? NSNull(),
                    "appId": row["app_id"] ?? NSNull(),
                    "params": jsonValue(row["params_json"] as? String),
                    "createdAt": row["created_at"] ?? NSNull(),
                ]
            },
        ]
    }

    private func consoleLogRows(appId: String?) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        let sql = appId == nil
            ? "SELECT bridge_call_id, app_id, params_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at LIMIT 100"
            : "SELECT bridge_call_id, app_id, params_json, created_at FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at LIMIT 100"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        if let appId {
            bind(statement, 1, appId)
        }
        var rows: [[String: Any]] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            rows.append([
                "bridge_call_id": columnText(statement, 0),
                "app_id": columnNullableText(statement, 1) ?? NSNull(),
                "params_json": columnNullableText(statement, 2) ?? NSNull(),
                "created_at": columnText(statement, 3),
            ])
        }
        return rows
    }

    private func notificationCapture(appId: String?) -> [String: Any] {
        [
            "appId": appId ?? NSNull(),
            "notifications": bridgeCallRows(appId: appId)
                .filter { ($0["method"] as? String) == "notification.toast" }
                .map { row in
                    let params = jsonDictionary(row["params_json"] as? String ?? "") ?? [:]
                    return [
                        "bridgeCallId": row["bridge_call_id"] ?? NSNull(),
                        "appId": row["app_id"] ?? NSNull(),
                        "message": params["message"] ?? NSNull(),
                        "level": params["level"] ?? NSNull(),
                        "params": params,
                        "createdAt": row["created_at"] ?? NSNull(),
                    ]
                },
        ]
    }

    private func clearRuntimeLogs(appId: String?) -> [String: Any] {
        if let appId {
            return [
                "ok": true,
                "appId": appId,
                "bridgeCallsCleared": deleteRows("DELETE FROM bridge_calls WHERE app_id = ?", values: [appId]),
                "coreEventsCleared": deleteRows("DELETE FROM core_events WHERE app_id = ?", values: [appId]),
                "coreActionsCleared": deleteRows("DELETE FROM core_actions WHERE app_id = ?", values: [appId]),
            ]
        }
        return [
            "ok": true,
            "appId": NSNull(),
            "bridgeCallsCleared": deleteRows("DELETE FROM bridge_calls"),
            "coreEventsCleared": deleteRows("DELETE FROM core_events"),
            "coreActionsCleared": deleteRows("DELETE FROM core_actions"),
        ]
    }

    private func exportDebugBundle() -> [String: Any] {
        let exportId = "export_\(UUID().uuidString.lowercased())"
        let createdAt = Self.now()
        var document: [String: Any] = [
            "exportId": exportId,
            "type": "debug-bundle",
            "createdAt": createdAt,
            "runtimeVersion": "0.4.0",
            "source": [
                "platform": "macos",
                "target": "macos",
            ],
            "apps": tableRows(
                table: "apps",
                columns: ["id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"],
                orderBy: "id"
            ),
            "appVersions": tableRows(
                table: "app_versions",
                columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "manifest_json", "content_hash", "status", "created_at", "activated_at"],
                orderBy: "created_at"
            ),
            "appFiles": tableRows(
                table: "app_files",
                columns: ["install_id", "path", "content_text", "content_hash", "size_bytes", "mime", "created_at"],
                orderBy: "path"
            ),
            "appPermissions": tableRows(
                table: "app_permissions",
                columns: ["install_id", "app_id", "permission", "requested", "approved", "approved_at", "reason"],
                orderBy: "permission"
            ),
            "appStorage": tableRows(
                table: "app_storage",
                columns: ["app_id", "key", "value_json", "updated_at"],
                orderBy: "updated_at"
            ),
            "appInstallReports": tableRows(
                table: "app_install_reports",
                columns: ["report_id", "app_id", "install_id", "status", "validation_json", "security_json", "permissions_json", "compatibility_json", "smoke_test_json", "content_hash", "created_at"],
                orderBy: "created_at"
            ),
            "runtimeCapabilities": runtimeCapabilities(appId: nil),
            "debug": [
                "runtimeSessions": tableRows(
                    table: "runtime_sessions",
                    columns: ["session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"],
                    orderBy: "started_at"
                ),
                "bridgeCalls": tableRows(
                    table: "bridge_calls",
                    columns: ["bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"],
                    orderBy: "created_at"
                ),
                "controlSessions": tableRows(
                    table: "control_sessions",
                    columns: ["control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"],
                    orderBy: "started_at"
                ),
                "controlCommands": controlCommands(),
                "coreEvents": tableRows(
                    table: "core_events",
                    columns: ["event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"],
                    orderBy: "created_at"
                ),
                "coreActions": tableRows(
                    table: "core_actions",
                    columns: ["action_id", "event_id", "session_id", "app_id", "action_json", "created_at"],
                    orderBy: "created_at"
                ),
                "runtimeSnapshots": tableRows(
                    table: "runtime_snapshots",
                    columns: ["snapshot_id", "session_id", "app_id", "install_id", "type", "snapshot_json", "content_hash", "created_at"],
                    orderBy: "created_at"
                ),
                "testRuns": tableRows(
                    table: "test_runs",
                    columns: ["test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at", "result_json", "diagnostics_json"],
                    orderBy: "started_at"
                ),
            ],
        ]
        let contentHash = "sha256:\(sha256Hex(jsonBody(document)))"
        document["contentHash"] = contentHash
        recordBackupExport(document, contentHash: contentHash, createdAt: createdAt)
        return document
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

    private func controlStorageContext(appId: String, permissions: Set<String>) -> AppSandboxContext {
        AppSandboxContext(
            appId: appId,
            approvedPermissions: permissions,
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: controlSessionId
        )
    }

    private func dispatchControlBridge(
        appId: String,
        method: String,
        params: [String: Any],
        requestId: String,
        startedAt: Date
    ) -> BridgeResponse {
        let manifest = manifestForApp(appId)
        let context = AppSandboxContext(
            appId: appId,
            storagePrefix: manifest["storagePrefix"] as? String,
            approvedPermissions: manifestPermissions(manifest),
            networkPolicy: NetworkPolicyRule.fromManifest(manifest),
            denyPrivateNetwork: manifestDenyPrivateNetwork(manifest),
            mountToken: controlSessionId
        )
        let request = BridgeRequest(id: requestId, method: method, params: params, context: context)
        let response: BridgeResponse
        if let fault = takeInjectedFault(appId: appId, sessionId: activeRuntimeSessionId, method: method) {
            response = .failure(
                id: requestId,
                code: fault.code,
                message: fault.message,
                details: fault.details
            )
        } else if let permission = permissionForBridgeMethod(method),
           !context.approvedPermissions.contains(permission) {
            response = .failure(
                id: requestId,
                code: "permission_denied",
                message: "App \(appId) cannot call \(method)",
                details: ["appId": appId, "method": method, "requiredPermission": permission]
            )
        } else {
            response = dispatchAllowedControlBridge(request)
        }
        recordBridgeCall(appId: appId, method: method, params: params, response: response, startedAt: startedAt)
        if method == "core.step", response.ok, let event = params["event"], let result = response.result {
            recordCoreStep(appId: appId, event: event, result: result)
        }
        return response
    }

    private func dispatchAllowedControlBridge(_ request: BridgeRequest) -> BridgeResponse {
        switch request.method {
        case "storage.get":
            return PlatformStorage(databaseURL: databaseURL).get(request)
        case "storage.set":
            return PlatformStorage(databaseURL: databaseURL).set(request)
        case "storage.remove":
            return PlatformStorage(databaseURL: databaseURL).remove(request)
        case "storage.list":
            return PlatformStorage(databaseURL: databaseURL).list(request)
        case "notification.toast":
            return PlatformNotifications().toast(request)
        case "network.request":
            return mockedNetworkResponse(request) ?? PlatformNetwork().request(request)
        case "core.step":
            return core.step(request)
        case "runtime.capabilities":
            return .success(id: request.id, result: runtimeCapabilities(appId: request.context.appId))
        case "app.log":
            NSLog("Generated app log: \(request.params)")
            return .success(id: request.id, result: ["ok": true])
        case "dialog.openFile", "dialog.saveFile":
            return mockedDialogResponse(request)
                ?? .failure(id: request.id, code: "platform_unsupported", message: "\(request.method) requires an interactive macOS dialog")
        default:
            return .failure(id: request.id, code: "unknown_method", message: "Unknown bridge method: \(request.method)")
        }
    }

    private func mockedNetworkResponse(_ request: BridgeRequest) -> BridgeResponse? {
        guard let urlText = request.params["url"] as? String,
              let url = URL(string: urlText),
              let origin = PlatformNetwork.origin(for: url)
        else {
            return nil
        }
        if request.context.denyPrivateNetwork && PlatformNetwork.isPrivateNetworkHost(url.host) {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request private network targets are denied")
        }
        if let credentials = request.params["credentials"], !(credentials is NSNull) {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request credentials are not allowed")
        }
        let method = (request.params["method"] as? String ?? "GET").uppercased()
        guard let headers = stringHeaders(request.params["headers"]) else {
            return .failure(id: request.id, code: "invalid_request", message: "network.request headers must be strings")
        }
        guard let bodyBytes = bodyByteCount(request.params["body"]) else {
            return .failure(id: request.id, code: "invalid_request", message: "network.request body must be a string or null")
        }
        guard let rule = request.context.networkPolicy.first(where: { $0.allows(origin: origin, method: method, headers: Array(headers.keys)) }) else {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request is not allowed by manifest.networkPolicy")
        }
        if bodyBytes > rule.maxRequestBytes {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request body exceeds manifest.networkPolicy maxRequestBytes")
        }
        guard let mock = findNetworkMock(
            sessionId: activeRuntimeSessionId,
            appId: request.context.appId,
            method: method,
            url: urlText
        ) else {
            return nil
        }
        let response = networkResponsePayload(mock)
        if responseByteCount(response) > rule.maxResponseBytes {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.response exceeds manifest.networkPolicy maxResponseBytes")
        }
        return .success(id: request.id, result: response)
    }

    private func mockedDialogResponse(_ request: BridgeRequest) -> BridgeResponse? {
        guard let dialogType = normalizedDialogType(["method": request.method]) else {
            return nil
        }
        if let mock = findDialogMock(sessionId: activeRuntimeSessionId, appId: request.context.appId, dialogType: dialogType) {
            return .success(id: request.id, result: mock)
        }
        if dialogType == "saveFile" {
            return .success(id: request.id, result: ["ok": true])
        }
        return nil
    }

    private func normalizedDialogType(_ args: [String: Any]) -> String? {
        let raw = args["dialogType"] as? String ?? (args["method"] as? String)?.replacingOccurrences(of: "dialog.", with: "")
        guard raw == "openFile" || raw == "saveFile" else {
            return nil
        }
        return raw
    }

    private func methodForFaultKind(_ kind: String?) -> String? {
        switch kind {
        case "storage.read":
            return "storage.get"
        case "storage.write":
            return "storage.set"
        case "network", "network.request":
            return "network.request"
        case "core", "core.step":
            return "core.step"
        case let value?:
            return value
        case nil:
            return nil
        }
    }

    private func faultDetails(kind: String?) -> [String: Any] {
        guard let kind else {
            return [:]
        }
        return ["kind": kind]
    }

    private func isKnownControlBridgeMethod(_ method: String) -> Bool {
        switch method {
        case "storage.get", "storage.set", "storage.remove", "storage.list",
             "notification.toast", "network.request", "core.step",
             "runtime.capabilities", "app.log", "dialog.openFile", "dialog.saveFile":
            return true
        default:
            return false
        }
    }

    private func permissionForBridgeMethod(_ method: String) -> String? {
        switch method {
        case "storage.get", "storage.list":
            return "storage.read"
        case "storage.set", "storage.remove":
            return "storage.write"
        case "dialog.openFile", "dialog.saveFile", "notification.toast", "network.request", "core.step":
            return method
        default:
            return nil
        }
    }

    private func manifestForApp(_ appId: String) -> [String: Any] {
        if let manifest = activeManifest(appId: appId),
           manifest["permissions"] != nil || manifest["networkPolicy"] != nil {
            return manifest
        }
        let manifestURL = RuntimeResourceLocator.repoRootURL()
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent("manifest.json")
        guard let data = try? Data(contentsOf: manifestURL),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return [:]
        }
        return object
    }

    private func activeManifest(appId: String) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = """
        SELECT v.manifest_json
        FROM apps a
        JOIN app_versions v ON v.install_id = a.active_install_id
        WHERE a.id = ?
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return jsonDictionary(columnText(statement, 0))
    }

    private func manifestPermissions(_ manifest: [String: Any]) -> Set<String> {
        Set(manifest["permissions"] as? [String] ?? [])
    }

    private func manifestDenyPrivateNetwork(_ manifest: [String: Any]) -> Bool {
        guard let policy = manifest["networkPolicy"] as? [String: Any] else {
            return true
        }
        return (policy["denyPrivateNetwork"] as? Bool) ?? true
    }

    private func recordBridgeCall(
        appId: String,
        method: String,
        params: [String: Any],
        response: BridgeResponse,
        startedAt: Date
    ) {
        guard let db = database.handle else { return }
        let sessionId = runtimeSessionId(appId: appId)
        let active = activeAppRecord(appId: appId)
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "bridge_\(UUID().uuidString.lowercased())")
        bind(statement, 2, sessionId)
        bind(statement, 3, appId)
        bindNullable(statement, 4, active?.installId)
        bind(statement, 5, method)
        bind(statement, 6, jsonBody(params))
        bindNullable(statement, 7, response.result.map(jsonString))
        bindNullable(statement, 8, response.error.map(jsonBody))
        sqlite3_bind_int64(statement, 9, Int64(Date().timeIntervalSince(startedAt) * 1000))
        bind(statement, 10, Self.now())
        sqlite3_step(statement)
    }

    private func recordCoreStep(appId: String, event: Any, result: Any) {
        guard let db = database.handle else { return }
        let sessionId = runtimeSessionId(appId: appId)
        let eventId = "core_event_\(UUID().uuidString.lowercased())"
        let resultObject = result as? [String: Any]
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, eventId)
        bind(statement, 2, sessionId)
        bind(statement, 3, appId)
        bindNullable(statement, 4, activeAppRecord(appId: appId)?.installId)
        bindNullableInt(statement, 5, stateVersionBefore(resultObject))
        bind(statement, 6, jsonString(event))
        bind(statement, 7, Self.now())
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return
        }
        for action in resultObject?["actions"] as? [[String: Any]] ?? [] {
            recordCoreAction(eventId: eventId, sessionId: sessionId, appId: appId, action: action)
        }
    }

    private func recordCoreAction(eventId: String, sessionId: String, appId: String, action: [String: Any]) {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "core_action_\(UUID().uuidString.lowercased())")
        bind(statement, 2, eventId)
        bind(statement, 3, sessionId)
        bind(statement, 4, appId)
        bind(statement, 5, jsonBody(action))
        bind(statement, 6, Self.now())
        sqlite3_step(statement)
    }

    private func stateVersionBefore(_ result: [String: Any]?) -> Int? {
        guard let value = intValue(result?["stateVersion"]) else {
            return nil
        }
        return max(0, value - 1)
    }

    private func runtimeSessionId(appId: String) -> String {
        let sessionId = "runtime_\(controlSessionId)"
        activeRuntimeSessionId = sessionId
        activeAppId = appId
        guard let db = database.handle else { return sessionId }
        let active = activeAppRecord(appId: appId)
        let now = Self.now()
        var statement: OpaquePointer?
        let sql = """
        INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json)
        VALUES (?, 'macos', 'macos', '0.1.0', ?, ?, ?, 'running', ?, '{"source":"native-macos-control"}')
        ON CONFLICT(session_id) DO UPDATE SET active_app_id = excluded.active_app_id, active_install_id = excluded.active_install_id, status = 'running'
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return sessionId
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, sessionId)
        bind(statement, 2, appId)
        bindNullable(statement, 3, active?.installId)
        bind(statement, 4, now)
        bind(statement, 5, jsonBody(runtimeCapabilities(appId: appId)))
        sqlite3_step(statement)
        return sessionId
    }

    private func recordBackupExport(_ document: [String: Any], contentHash: String, createdAt: String) {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        let sql = """
        INSERT OR REPLACE INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at)
        VALUES (?, 'debug-bundle', 'macos', '0.4.0', ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, document["exportId"] as? String ?? "")
        bind(statement, 2, jsonBody(document))
        bind(statement, 3, contentHash)
        bind(statement, 4, createdAt)
        sqlite3_step(statement)
    }

    private func activeAppRecord(appId: String) -> (installId: String?, version: String?, dataVersion: Int)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT active_install_id, active_version, data_version FROM apps WHERE id = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            installId: columnNullableText(statement, 0),
            version: columnNullableText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2))
        )
    }

    private func deleteStorage(appId: String) -> Bool {
        guard let db = database.handle else { return false }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM app_storage WHERE app_id = ?", -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func insertStorageRow(_ item: [String: Any], fallbackAppId: String?) -> Bool {
        guard let db = database.handle else { return false }
        let rowAppId = item["app_id"] as? String ?? item["appId"] as? String ?? fallbackAppId
        guard let appId = rowAppId, let key = item["key"] as? String else {
            return false
        }
        let valueJSON = item["value_json"] as? String ?? item["valueJson"] as? String ?? "null"
        var statement: OpaquePointer?
        let sql = """
        INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at)
        VALUES (?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, key)
        bind(statement, 3, valueJSON)
        bind(statement, 4, Self.now())
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func updateActiveAppAfterRestore(
        appId: String,
        activeInstallId: String,
        activeVersion: String?,
        dataVersion: Int?
    ) -> Bool {
        guard let db = database.handle else { return false }
        var statement: OpaquePointer?
        let sql = """
        UPDATE apps
        SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ?
        WHERE id = ?
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, activeInstallId)
        bindNullable(statement, 2, activeVersion)
        sqlite3_bind_int64(statement, 3, Int64(dataVersion ?? 1))
        bind(statement, 4, Self.now())
        bind(statement, 5, appId)
        return sqlite3_step(statement) == SQLITE_DONE
    }

    private func executeSQL(_ sql: String) -> Bool {
        guard let db = database.handle else { return false }
        var error: UnsafeMutablePointer<CChar>?
        let status = sqlite3_exec(db, sql, nil, nil, &error)
        sqlite3_free(error)
        return status == SQLITE_OK
    }

    private func deleteRows(_ sql: String, values: [String] = []) -> Int {
        guard let db = database.handle else { return 0 }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bind(statement, Int32(index + 1), value)
        }
        guard sqlite3_step(statement) == SQLITE_DONE else {
            return 0
        }
        return Int(sqlite3_changes(db))
    }

    private func jsonDictionary(_ text: String) -> [String: Any]? {
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        return object
    }

    private func jsonString(_ value: Any) -> String {
        if let object = value as? [String: Any] {
            return jsonBody(object)
        }
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return jsonBody(["value": value])
        }
        return string
    }

    private func jsonValue(_ text: String?) -> Any {
        guard let text,
              let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data)
        else {
            return NSNull()
        }
        return object
    }

    private func canonicalJSONEqual(_ left: Any, _ right: Any) -> Bool {
        jsonString(left) == jsonString(right)
    }

    private func stringHeaders(_ value: Any?) -> [String: String]? {
        guard let value, !(value is NSNull) else {
            return [:]
        }
        guard let raw = value as? [String: Any] else {
            return nil
        }
        var headers: [String: String] = [:]
        for (name, headerValue) in raw {
            guard let text = headerValue as? String else {
                return nil
            }
            headers[name.lowercased()] = text
        }
        return headers
    }

    private func bodyByteCount(_ value: Any?) -> Int? {
        guard let value, !(value is NSNull) else {
            return 0
        }
        guard let text = value as? String else {
            return nil
        }
        return text.utf8.count
    }

    private func networkResponsePayload(_ value: Any) -> Any {
        guard var object = value as? [String: Any] else {
            return value
        }
        object.removeValue(forKey: "delayMs")
        return object
    }

    private func responseByteCount(_ value: Any) -> Int {
        guard let object = value as? [String: Any] else {
            return jsonString(value).utf8.count
        }
        if let text = object["bodyText"] as? String {
            return text.utf8.count
        }
        if let text = object["body"] as? String {
            return text.utf8.count
        }
        if let body = object["body"], !(body is NSNull) {
            return jsonString(body).utf8.count
        }
        return 0
    }

    private func urlMatches(pattern: String, url: String) -> Bool {
        if pattern == url || pattern == "*" {
            return true
        }
        let escaped = NSRegularExpression.escapedPattern(for: pattern)
            .replacingOccurrences(of: #"\\\*"#, with: ".*", options: .regularExpression)
        return url.range(of: #"^\#(escaped)$"#, options: .regularExpression) != nil
    }

    private func intValue(_ value: Any?) -> Int? {
        if let value = value as? Int {
            return value
        }
        if let value = value as? NSNumber {
            return value.intValue
        }
        if let value = value as? String {
            return Int(value)
        }
        return nil
    }

    private func scalarInt(_ sql: String, values: [String] = []) -> Int {
        guard let db = database.handle else { return 0 }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
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

    private func tableRows(
        table: String,
        columns: [String],
        orderBy: String,
        filterColumn: String? = nil,
        filterValue: String? = nil
    ) -> [[String: Any]] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let filterSQL = filterColumn != nil && filterValue != nil ? " WHERE \(filterColumn!) = ?" : ""
        let sql = "SELECT \(columns.joined(separator: ", ")) FROM \(table)\(filterSQL) ORDER BY \(orderBy) LIMIT 100"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        if let filterValue {
            bind(statement, 1, filterValue)
        }

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

    private func htmlForBundledApp(_ appId: String) -> String {
        let htmlURL = RuntimeResourceLocator.repoRootURL()
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent("index.html")
        return (try? String(contentsOf: htmlURL, encoding: .utf8)) ?? ""
    }

    private func runtimeQuery(appId: String, args: [String: Any]) -> [String: Any] {
        let html = htmlForBundledApp(appId)
        let query: String
        if let testId = args["testId"] as? String {
            query = #"[data-testid="\#(testId)"]"#
        } else {
            query = args["selector"] as? String ?? args["text"] as? String ?? ""
        }
        let matches = queryMatches(html: html, args: args)
        return [
            "ok": !matches.isEmpty,
            "appId": appId,
            "query": query,
            "matches": matches,
        ]
    }

    private func runtimeScreenshot(appId: String, label: String?) -> [String: Any] {
        let html = htmlForBundledApp(appId)
        let text = htmlText(html)
        return [
            "ok": true,
            "appId": appId,
            "label": label ?? NSNull(),
            "format": "static-html-summary",
            "title": firstMatch(in: html, pattern: #"<title[^>]*>([\s\S]*?)</title>"#),
            "textHash": "sha256:\(sha256Hex(text))",
            "testIds": testIds(in: html),
        ]
    }

    private func queryMatches(html: String, args: [String: Any]) -> [[String: Any]] {
        if let testId = args["testId"] as? String, let tag = tagForAttribute(html: html, attr: "data-testid", value: testId) {
            return [["kind": "testId", "value": testId, "tag": tag]]
        }
        if let selector = args["selector"] as? String, selector.hasPrefix("#") {
            let id = String(selector.dropFirst())
            if let tag = tagForAttribute(html: html, attr: "id", value: id) {
                return [["kind": "selector", "value": selector, "tag": tag]]
            }
        }
        if let selector = args["selector"] as? String,
           let testId = testIdSelectorValue(selector),
           let tag = tagForAttribute(html: html, attr: "data-testid", value: testId)
        {
            return [["kind": "selector", "value": selector, "tag": tag]]
        }
        if let text = args["text"] as? String, htmlText(html).contains(text) {
            return [["kind": "text", "value": text]]
        }
        if let selector = args["selector"] as? String, isSimpleTagSelector(selector) {
            let escaped = NSRegularExpression.escapedPattern(for: selector.lowercased())
            if html.range(of: #"<\#(escaped)\b"#, options: [.regularExpression, .caseInsensitive]) != nil {
                return [["kind": "selector", "value": selector, "tag": selector.lowercased()]]
            }
        }
        return []
    }

    private func testIds(in html: String) -> [String] {
        regexMatches(in: html, pattern: #"\bdata-testid=["']([^"']+)["']"#)
            .compactMap { $0[safe: 1] }
            .sorted()
    }

    private func tagForAttribute(html: String, attr: String, value: String) -> String? {
        let escapedAttr = NSRegularExpression.escapedPattern(for: attr)
        let escapedValue = NSRegularExpression.escapedPattern(for: value)
        return regexMatches(
            in: html,
            pattern: #"<([a-z0-9-]+)\b[^>]*\b\#(escapedAttr)=["']\#(escapedValue)["'][^>]*>"#
        ).first?[safe: 1]?.lowercased()
    }

    private func testIdSelectorValue(_ selector: String) -> String? {
        regexMatches(in: selector, pattern: #"\[data-testid=["']([^"']+)["']\]"#).first?[safe: 1]
    }

    private func isSimpleTagSelector(_ selector: String) -> Bool {
        selector.range(of: #"^[a-z][a-z0-9-]*$"#, options: [.regularExpression, .caseInsensitive]) != nil
    }

    private func headingRecords(_ html: String) -> [[String: Any]] {
        regexMatches(in: html, pattern: #"<h([1-6])\b[^>]*>([\s\S]*?)</h\1>"#).map { match in
            [
                "level": Int(match[safe: 1] ?? "") ?? 0,
                "name": htmlText(match[safe: 2] ?? ""),
            ]
        }
    }

    private func controlRecords(_ html: String) -> [[String: Any]] {
        var controls: [[String: Any]] = []
        for match in regexMatches(in: html, pattern: #"<(button|select|textarea|a)\b([^>]*)>([\s\S]*?)</\1>"#) {
            let tag = (match[safe: 1] ?? "").lowercased()
            let attrs = parseAttrs(match[safe: 2] ?? "")
            controls.append(controlRecord(html: html, tag: tag, attrs: attrs, innerHtml: match[safe: 3] ?? ""))
        }
        for match in regexMatches(in: html, pattern: #"<input\b([^>]*)>"#) {
            let attrs = parseAttrs(match[safe: 1] ?? "")
            if (attrs["type"] ?? "text").lowercased() == "hidden" {
                continue
            }
            controls.append(controlRecord(html: html, tag: "input", attrs: attrs, innerHtml: ""))
        }
        return controls.sorted { left, right in
            (left["selector"] as? String ?? "") < (right["selector"] as? String ?? "")
        }
    }

    private func controlRecord(html: String, tag: String, attrs: [String: String], innerHtml: String) -> [String: Any] {
        let testId = attrs["data-testid"] ?? ""
        let id = attrs["id"] ?? ""
        let selector = !testId.isEmpty ? #"[data-testid="\#(testId)"]"# : !id.isEmpty ? "#\(id)" : tag
        return [
            "tag": tag,
            "type": attrs["type"] ?? NSNull(),
            "testId": testId,
            "selector": selector,
            "name": accessibleName(html: html, tag: tag, attrs: attrs, innerHtml: innerHtml),
        ]
    }

    private func accessibleName(html: String, tag: String, attrs: [String: String], innerHtml: String) -> String {
        for attr in ["aria-label", "title"] {
            if let value = attrs[attr]?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty {
                return value
            }
        }
        if (tag == "button" || tag == "a") {
            let text = htmlText(innerHtml)
            if !text.isEmpty {
                return text
            }
        }
        if let id = attrs["id"], !id.isEmpty {
            if let label = labelForId(html: html, id: id), !label.isEmpty {
                return label
            }
            if let label = wrappingLabelForControl(html: html, tag: tag, id: id), !label.isEmpty {
                return label
            }
        }
        return ""
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

    private func bindNullableInt(_ statement: OpaquePointer?, _ index: Int32, _ value: Int?) {
        guard let value else {
            sqlite3_bind_null(statement, index)
            return
        }
        sqlite3_bind_int64(statement, index, Int64(value))
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
        case ("POST", "/db/app-storage"):
            return "db.query_app_storage"
        case ("POST", "/db/app-versions"):
            return "db.query_app_versions"
        case ("POST", "/db/bridge-calls"):
            return "db.query_bridge_calls"
        case ("POST", "/db/core-events"):
            return "db.query_core_events"
        case ("POST", "/db/test-runs"):
            return "db.query_test_runs"
        case ("POST", "/db/export-debug-bundle"):
            return "db.export_debug_bundle"
        case ("GET", "/apps"):
            return "platform.list_webapps"
        case ("GET", let value) where value.hasSuffix("/versions"):
            return "platform.list_webapp_versions"
        case ("POST", let value) where value.hasSuffix("/rollback"):
            return "platform.rollback_webapp"
        case ("GET", let value) where value.hasSuffix("/install-report"):
            return "platform.install_report"
        case ("DELETE", _):
            return "platform.stop"
        case ("GET", let value) where value.hasSuffix("/snapshot"):
            return "runtime.snapshot"
        case ("GET", let value) where value.hasSuffix("/events"):
            return "runtime.event_log"
        case ("GET", let value) where value.hasSuffix("/capabilities"):
            return "runtime.capabilities"
        case ("GET", let value) where value.hasSuffix("/resource-usage"):
            return "runtime.resource_usage"
        case ("GET", let value) where value.hasSuffix("/accessibility"):
            return "runtime.run_accessibility_audit"
        case ("POST", let value) where value.hasSuffix("/snapshots"):
            return "platform.create_snapshot"
        case ("POST", let value) where value.contains("/snapshots/"):
            if (jsonBody?["action"] as? String) == "restore" {
                return "platform.restore_snapshot"
            }
            return "runtime.snapshot"
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
    let itemId: String?

    init?(_ path: String) {
        let parts = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard parts.count >= 2, parts[0] == "sessions" else {
            return nil
        }
        self.controlSessionId = parts[1]
        self.subresource = parts.count > 2 ? parts[2] : nil
        self.itemId = parts.count > 3 ? parts[3] : nil
    }
}

private struct AppRoute {
    let appId: String
    let subresource: String

    init?(_ path: String) {
        let parts = path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard parts.count == 3, parts[0] == "apps" else {
            return nil
        }
        self.appId = parts[1]
        self.subresource = parts[2]
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

private func firstMatch(in text: String, pattern: String) -> String {
    regexMatches(in: text, pattern: pattern).first?[safe: 1].map(htmlText) ?? ""
}

private func regexMatches(in text: String, pattern: String) -> [[String]] {
    guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) else {
        return []
    }
    let range = NSRange(text.startIndex..<text.endIndex, in: text)
    return regex.matches(in: text, range: range).map { match in
        (0..<match.numberOfRanges).map { index in
            let matchRange = match.range(at: index)
            guard let range = Range(matchRange, in: text) else {
                return ""
            }
            return String(text[range])
        }
    }
}

private func parseAttrs(_ attrsText: String) -> [String: String] {
    var attrs: [String: String] = [:]
    for match in regexMatches(in: attrsText, pattern: #"\b([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#) {
        guard let name = match[safe: 1]?.lowercased() else {
            continue
        }
        if let value = match[safe: 2], !value.isEmpty {
            attrs[name] = value
        } else if let value = match[safe: 3], !value.isEmpty {
            attrs[name] = value
        } else {
            attrs[name] = match[safe: 4] ?? ""
        }
    }
    return attrs
}

private func htmlText(_ html: String) -> String {
    html.replacingOccurrences(of: #"<script\b[\s\S]*?</script>"#, with: " ", options: [.regularExpression, .caseInsensitive])
        .replacingOccurrences(of: #"<style\b[\s\S]*?</style>"#, with: " ", options: [.regularExpression, .caseInsensitive])
        .replacingOccurrences(of: #"<[^>]+>"#, with: " ", options: [.regularExpression])
        .replacingOccurrences(of: "&nbsp;", with: " ")
        .replacingOccurrences(of: "&amp;", with: "&")
        .replacingOccurrences(of: "&lt;", with: "<")
        .replacingOccurrences(of: "&gt;", with: ">")
        .replacingOccurrences(of: "&quot;", with: "\"")
        .replacingOccurrences(of: #"[\s\n\r\t]+"#, with: " ", options: [.regularExpression])
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func labelForId(html: String, id: String) -> String? {
    let escaped = NSRegularExpression.escapedPattern(for: id)
    return firstMatch(in: html, pattern: #"<label\b[^>]*\bfor=["']\#(escaped)["'][^>]*>([\s\S]*?)</label>"#)
}

private func wrappingLabelForControl(html: String, tag: String, id: String) -> String? {
    let escaped = NSRegularExpression.escapedPattern(for: id)
    let raw = firstMatch(in: html, pattern: #"<label\b[^>]*>([\s\S]*?<\#(tag)\b[^>]*\bid=["']\#(escaped)["'][^>]*>[\s\S]*?)</label>"#)
    guard !raw.isEmpty else {
        return nil
    }
    let beforeControl = raw.replacingOccurrences(of: #"<\#(tag)\b[\s\S]*"#, with: "", options: [.regularExpression, .caseInsensitive])
    return htmlText(beforeControl)
}

private func accessibilityCheck(id: String, ok: Bool, message: String, selector: String? = nil) -> [String: Any] {
    var check: [String: Any] = [
        "id": id,
        "status": ok ? "pass" : "fail",
        "message": message,
    ]
    if let selector, !selector.isEmpty {
        check["selector"] = selector
    }
    return check
}

private func sha256Hex(_ text: String) -> String {
    let digest = SHA256.hash(data: Data(text.utf8))
    return digest.map { String(format: "%02x", $0) }.joined()
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
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
