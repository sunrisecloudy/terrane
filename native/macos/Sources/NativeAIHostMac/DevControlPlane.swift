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
        var signingKeyAccount = "native-ai-webapp.macos.dev-control.platform-key"

        static func defaultConfiguration() -> Configuration {
            Configuration(
                port: UInt16(ProcessInfo.processInfo.environment["NATIVE_AI_MACOS_CONTROL_PORT"] ?? ""),
                tokenFileURL: defaultTokenFileURL(),
                databaseURL: nil,
                tokenOverride: nil,
                signingKeyAccount: ProcessInfo.processInfo.environment["NATIVE_AI_MACOS_SIGNING_KEY_ACCOUNT"]
                    ?? "native-ai-webapp.macos.dev-control.platform-key"
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
        case signingKeyUnavailable(OSStatus)
    }

    private struct InjectedFault {
        let code: String
        let message: String
        let details: [String: Any]
    }

    private struct PackageFile {
        let path: String
        let content: String
        let contentHash: String
        let sizeBytes: Int
        let mime: String
    }

    private struct PackageRead {
        let directory: URL
        let manifest: [String: Any]
        let manifestJSON: String
        let files: [PackageFile]
        let errors: [[String: Any]]
        let warnings: [[String: Any]]
    }

    let token: String
    let tokenFileURL: URL
    let controlSessionId: String

    private let database: PlatformDatabase
    private let databaseURL: URL?
    private let core = ZigCoreBridge()
    private let crdt = ZigCrdtBridge()
    private let signingKey: Curve25519.Signing.PrivateKey
    private let signingKeyAccount: String
    private let queue = DispatchQueue(label: "dev.nativeai.macos.control-plane")
    private var listener: NWListener?
    private var sessionStatus = "running"
    private var activeRuntimeSessionId: String?
    private var activeAppId: String?
    private static let signingKeyService = "native-ai-webapp.macos.dev-control"
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

    var platformSigningKeyId: String {
        signingKeyId()
    }

    static func deleteSigningKeyForTests(account: String) {
        _ = SecItemDelete(signingKeyQuery(account: account) as CFDictionary)
    }

    init(configuration: Configuration = .defaultConfiguration()) throws {
        self.token = try configuration.tokenOverride ?? Self.generateToken()
        self.tokenFileURL = configuration.tokenFileURL
        self.controlSessionId = "control_\(UUID().uuidString.lowercased())"
        self.databaseURL = configuration.databaseURL
        self.database = PlatformDatabase(databaseURL: configuration.databaseURL)
        self.signingKeyAccount = configuration.signingKeyAccount
        self.signingKey = try Self.loadOrCreateSigningKey(account: configuration.signingKeyAccount)
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
        case "runtime.replay_events":
            handleRuntimeReplayEvents(connection, request, args: args, startedAt: startedAt)
        case "runtime.assert_core_action":
            handleCoreActionAssertion(connection, request, args: args, startedAt: startedAt)
        case "runtime.compare_snapshot":
            handleRuntimeCompareSnapshot(connection, request, args: args, startedAt: startedAt)
        case "runtime.run_smoke_tests":
            handleRuntimeRunSmokeTests(connection, request, args: args, startedAt: startedAt)
        case "runtime.run_microtest":
            handleRuntimeRunMicrotest(connection, request, args: args, startedAt: startedAt)
        case "runtime.accessibility_snapshot":
            sendAccepted(connection, request, startedAt: startedAt, result: accessibilitySnapshot(appId: args["appId"] as? String))
        case "runtime.run_accessibility_audit":
            sendAccepted(connection, request, startedAt: startedAt, result: accessibilityAudit(appId: args["appId"] as? String))
        case "runtime.assert_accessibility":
            handleAccessibilityAssertion(connection, request, args: args, startedAt: startedAt)
        case "platform.validate_package", "platform.run_policy_audit":
            handleValidatePackage(connection, request, args: args, startedAt: startedAt)
        case "platform.sign_webapp_package":
            handleSignPackage(connection, request, args: args, startedAt: startedAt)
        case "platform.install_webapp_package":
            handleInstallPackage(connection, request, args: args, startedAt: startedAt)
        case "platform.run_platform_smoke":
            handleRunPlatformSmoke(connection, request, args: args, startedAt: startedAt)
        case "platform.list_webapps":
            sendAccepted(connection, request, startedAt: startedAt, result: listWebapps(includeUninstalled: args["includeUninstalled"] as? Bool == true))
        case "platform.uninstall_webapp":
            handleUninstallWebapp(connection, request, args: args, startedAt: startedAt)
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
        case "platform.quarantine_webapp":
            handleQuarantineWebapp(connection, request, args: args, startedAt: startedAt)
        case "platform.approve_webapp_update":
            handleApproveWebappUpdate(connection, request, args: args, startedAt: startedAt)
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
        case "platform.migration_dry_run":
            handleMigrationRun(connection, request, args: args, mode: "dry-run", startedAt: startedAt)
        case "platform.migration_apply":
            handleMigrationRun(connection, request, args: args, mode: "apply", startedAt: startedAt)
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
        case "db.export_backup":
            sendAccepted(connection, request, startedAt: startedAt, result: exportBackup(type: "backup", includeDebug: false))
        case "db.import_backup":
            handleImportBackup(connection, request, args: args, startedAt: startedAt)
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
            "keyId": signingKeyId(),
            "signingPublicKey": signingPublicKeyDescriptor(),
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
                    "id": "reference-host",
                    "platform": "reference-host",
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
        var limits: [String: Any] = [
            "maxPackageBytes": 1_048_576,
            "maxFileBytes": 524_288,
        ]
        if let appId {
            for (key, value) in AppSandboxContext.resourceBudget(from: manifestForApp(appId)) {
                limits[key] = value
            }
        }
        return [
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
                "notebook.crdt": crdt.smokeMaterialize(),
                "runtime.capabilities": true,
                "app.log": true,
            ],
            "limits": limits,
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
        guard let app = appRecord(appId: appId), app.status != "uninstalled" else {
            sendRejected(connection, request, status: 400, code: "app_not_installed", message: "App is not installed", startedAt: startedAt)
            return
        }
        guard app.status != "quarantined" else {
            sendRejected(connection, request, status: 400, code: "package_quarantined", message: "App is quarantined", startedAt: startedAt)
            return
        }
        guard app.activeInstallId != nil else {
            sendRejected(connection, request, status: 400, code: "app_not_installed", message: "App has no active install", startedAt: startedAt)
            return
        }
        if let installId = app.activeInstallId,
           let verificationError = verifyActiveInstallForMount(appId: appId, installId: installId) {
            sendRejected(
                connection,
                request,
                status: 400,
                code: verificationError.code,
                message: verificationError.message,
                startedAt: startedAt
            )
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

    private func handleQuarantineWebapp(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.quarantine_webapp requires appId", startedAt: startedAt)
            return
        }
        guard let result = quarantineWebapp(
            appId: appId,
            installId: args["installId"] as? String,
            reason: args["reason"] as? String ?? "manual quarantine",
            restorePrevious: args["restorePrevious"] as? Bool == true
        ) else {
            sendRejected(connection, request, status: 400, code: "quarantine_failed", message: "Quarantine could not be completed", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleApproveWebappUpdate(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.approve_webapp_update requires appId", startedAt: startedAt)
            return
        }
        guard let installId = args["installId"] as? String, !installId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.approve_webapp_update requires installId", startedAt: startedAt)
            return
        }
        guard let result = approveWebappUpdate(appId: appId, installId: installId) else {
            sendRejected(connection, request, status: 400, code: "approval_failed", message: "Update approval could not be completed", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleUninstallWebapp(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.uninstall_webapp requires appId", startedAt: startedAt)
            return
        }
        guard args["confirm"] as? Bool == true else {
            sendRejected(connection, request, status: 400, code: "confirmation_required", message: "platform.uninstall_webapp requires confirm: true", startedAt: startedAt)
            return
        }
        guard let result = uninstallWebapp(appId: appId) else {
            sendRejected(connection, request, status: 400, code: "app_not_installed", message: "App is not installed", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleImportBackup(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let backup = args["backup"] as? [String: Any] else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "db.import_backup requires backup", startedAt: startedAt)
            return
        }
        guard let result = importBackup(backup) else {
            sendRejected(connection, request, status: 400, code: "invalid_backup", message: "Backup import could not be completed", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
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
        guard args["confirm"] as? Bool == true else {
            sendRejected(connection, request, status: 400, code: "confirmation_required", message: "\(request.toolName) requires confirm: true", startedAt: startedAt)
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

    private func handleRuntimeReplayEvents(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.replay_events requires appId", startedAt: startedAt)
            return
        }
        guard let events = args["events"] as? [Any] else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.replay_events events must be an array", startedAt: startedAt)
            return
        }
        let replayCore = ZigCoreBridge()
        let context = AppSandboxContext(
            appId: appId,
            approvedPermissions: ["core.step"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: controlSessionId
        )
        let replay = events.enumerated().map { index, event in
            let response = replayCore.step(BridgeRequest(
                id: "control_replay_\(index)",
                method: "core.step",
                params: ["event": event],
                context: context
            ))
            return [
                "index": index,
                "event": event,
                "result": response.result ?? [
                    "ok": false,
                    "error": response.error ?? [
                        "code": "core_error",
                        "message": "Replay event failed",
                        "details": [:],
                    ],
                    "actions": [],
                ],
            ] as [String: Any]
        }
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": true,
            "appId": appId,
            "replay": replay,
        ])
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

    private func handleRuntimeCompareSnapshot(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let left = snapshotValue(args["left"], snapshotId: args["leftSnapshotId"] as? String),
              let right = snapshotValue(args["right"], snapshotId: args["rightSnapshotId"] as? String)
        else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.compare_snapshot requires left/right snapshots or snapshot ids", startedAt: startedAt)
            return
        }
        let leftJSON = jsonString(comparableSnapshotValue(left))
        let rightJSON = jsonString(comparableSnapshotValue(right))
        let equal = leftJSON == rightJSON
        sendAccepted(connection, request, startedAt: startedAt, result: [
            "ok": equal,
            "equal": equal,
            "leftHash": "sha256:\(sha256Hex(leftJSON))",
            "rightHash": "sha256:\(sha256Hex(rightJSON))",
        ])
    }

    private func handleRuntimeRunSmokeTests(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let appId = args["appId"] as? String, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.run_smoke_tests requires appId", startedAt: startedAt)
            return
        }
        guard activeAppRecord(appId: appId)?.installId != nil else {
            sendRejected(connection, request, status: 400, code: "app_not_installed", message: "App is not installed", startedAt: startedAt)
            return
        }
        guard let smokeText = bundledAppText(appId: appId, path: "smoke-tests.json") else {
            sendRejected(connection, request, status: 400, code: "smoke_tests_missing", message: "App has no smoke-tests.json: \(appId)", startedAt: startedAt)
            return
        }
        guard let tests = bundledSmokeTests(text: smokeText) else {
            sendRejected(connection, request, status: 400, code: "invalid_smoke_tests", message: "App smoke-tests.json is invalid: \(appId)", startedAt: startedAt)
            return
        }
        let result = evaluateSmokeTests(appId: appId, tests: tests)
        guard let run = recordTestRun(
            microTestId: "smoke:\(appId)",
            name: "\(appId) bundled smoke tests",
            appId: appId,
            spec: tests,
            status: (result["ok"] as? Bool) == true ? "passed" : "failed",
            result: result,
            diagnostics: ["runner": "native-macos-static"]
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Smoke test run could not be recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: run)
    }

    private func handleRuntimeRunMicrotest(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let microtest = microtestSpec(args: args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "runtime.run_microtest requires spec or microtestPath", startedAt: startedAt)
            return
        }
        guard let appId = (microtest["targetApps"] as? [String])?.first, !appId.isEmpty else {
            sendRejected(connection, request, status: 400, code: "invalid_microtest", message: "Micro-test must target at least one app", startedAt: startedAt)
            return
        }
        let setup = executeMicrotestPhase("setup", steps: microtest["setup"] as? [[String: Any]] ?? [], appId: appId)
        let result = evaluateMicroTest(appId: appId, microtest: microtest, commandResults: setup["commands"] as? [[String: Any]] ?? [])
        let teardown = executeMicrotestPhase("teardown", steps: microtest["teardown"] as? [[String: Any]] ?? [], appId: appId)
        let passed = (setup["ok"] as? Bool) == true && (result["ok"] as? Bool) == true && (teardown["ok"] as? Bool) == true
        var runResult = result
        runResult["setup"] = setup
        runResult["teardown"] = teardown
        guard let run = recordTestRun(
            microTestId: microtest["id"] as? String ?? "microtest:\(appId)",
            name: microtest["id"] as? String ?? "\(appId) micro-test",
            appId: appId,
            spec: microtest,
            status: passed ? "passed" : "failed",
            result: runResult,
            diagnostics: ["runner": "native-macos-static"]
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Micro-test run could not be recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: run)
    }

    private func handleValidatePackage(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let result = validatePackageResult(args: args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.validate_package requires packagePath or path", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleSignPackage(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let result = signPackageResult(args: args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.sign_webapp_package requires packagePath or path", startedAt: startedAt)
            return
        }
        guard (result["ok"] as? Bool) == true else {
            sendRejected(connection, request, status: 400, code: "package_validation_failed", message: "Generated webapp package failed validation", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleInstallPackage(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let result = installPackageResult(args: args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.install_webapp_package requires packagePath or path", startedAt: startedAt)
            return
        }
        guard (result["ok"] as? Bool) == true else {
            sendRejected(connection, request, status: 400, code: "package_validation_failed", message: "Generated webapp package failed validation", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: result)
    }

    private func handleRunPlatformSmoke(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], startedAt: Date) {
        guard let smoke = platformSmokeSpec(args: args) else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "platform.run_platform_smoke requires spec or smokePath", startedAt: startedAt)
            return
        }
        let platform = args["platform"] as? String ?? "macos"
        let result = runPlatformSmoke(smoke: smoke, platform: platform)
        guard let run = recordTestRun(
            microTestId: "platform-smoke:\(smoke["id"] as? String ?? "unnamed"):\(platform)",
            name: "\(smoke["id"] as? String ?? "platform smoke") (\(platform))",
            appId: nil,
            spec: smoke,
            status: (result["ok"] as? Bool) == true ? "passed" : "failed",
            result: result,
            diagnostics: ["runner": "native-macos-static"]
        ) else {
            sendRejected(connection, request, status: 400, code: "sqlite_error", message: "Platform smoke run could not be recorded", startedAt: startedAt)
            return
        }
        sendAccepted(connection, request, startedAt: startedAt, result: run)
    }

    private func handleMigrationRun(_ connection: NWConnection, _ request: HTTPRequest, args: [String: Any], mode: String, startedAt: Date) {
        guard let migration = args["migration"] as? [String: Any] else {
            sendRejected(connection, request, status: 400, code: "invalid_request", message: "\(request.toolName) requires migration", startedAt: startedAt)
            return
        }
        do {
            let result = try runMigration(migration: migration, mode: mode)
            sendAccepted(connection, request, startedAt: startedAt, result: result)
        } catch {
            sendRejected(connection, request, status: 400, code: "migration_failed", message: "\(request.toolName) could not complete", startedAt: startedAt)
        }
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

    private func snapshotValue(_ value: Any?, snapshotId: String?) -> Any? {
        if let snapshotId, !snapshotId.isEmpty {
            return readSnapshot(snapshotId: snapshotId)?["snapshot"]
        }
        if let value, !(value is NSNull) {
            return value
        }
        return nil
    }

    private func comparableSnapshotValue(_ value: Any) -> Any {
        guard var snapshot = value as? [String: Any] else {
            return value
        }
        snapshot.removeValue(forKey: "createdAt")
        snapshot.removeValue(forKey: "snapshotId")
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

    private func verifyActiveInstallForMount(appId: String, installId: String) -> (code: String, message: String)? {
        guard let version = installedVersionForMount(appId: appId, installId: installId) else {
            return ("app_not_installed", "Active install is missing")
        }
        if version.status == "quarantined" {
            return ("package_quarantined", "App version is quarantined")
        }

        let files = installedFiles(installId: installId)
        if files.isEmpty {
            return nil
        }
        guard let signature = version.signature else {
            return ("signature_missing", "Installed package has no signature")
        }

        guard textValue(signature, keys: ["algorithm"]) == "ed25519" else {
            return ("signature_untrusted", "Installed package signature algorithm is not trusted")
        }
        guard textValue(signature, keys: ["keyId"]) == signingKeyId() else {
            return ("signature_untrusted", "Installed package signature key is not trusted")
        }

        let hashes = packageHashes(
            manifest: version.manifest,
            files: files,
            permissions: permissionsForInstall(installId: installId)
        )
        guard textValue(signature, keys: ["manifestHash"]) == hashes["manifestHash"] else {
            return ("manifest_tampered", "Stored manifest hash does not match the signature")
        }
        guard textValue(signature, keys: ["contentHash"]) == hashes["contentHash"],
              version.contentHash == hashes["contentHash"]
        else {
            return ("content_tampered", "Stored app file content does not match the signature")
        }
        guard textValue(signature, keys: ["permissionsHash"]) == hashes["permissionsHash"] else {
            return ("permission_tampered", "Stored permissions hash does not match the signature")
        }
        guard textValue(signature, keys: ["policyHash"]) == hashes["policyHash"] else {
            return ("policy_tampered", "Stored policy hash does not match the signature")
        }
        guard textValue(signature, keys: ["runtimeVersion"]) == version.runtimeVersion,
              intValue(signature["dataVersion"]) == version.dataVersion
        else {
            return ("signature_invalid", "Installed package signature metadata does not match the active version")
        }

        guard let signatureText = textValue(signature, keys: ["signature"]),
              let signatureData = Data(base64Encoded: signatureText),
              let signedAt = textValue(signature, keys: ["signedAt"]),
              let trustLevel = textValue(signature, keys: ["trustLevel"]),
              let keyId = textValue(signature, keys: ["keyId"])
        else {
            return ("signature_invalid", "Installed package signature is malformed")
        }
        let payload = signaturePayload(
            appId: appId,
            appVersion: version.version,
            dataVersion: version.dataVersion,
            runtimeVersion: version.runtimeVersion,
            trustLevel: trustLevel,
            keyId: keyId,
            manifestHash: hashes["manifestHash"] ?? "",
            contentHash: hashes["contentHash"] ?? "",
            permissionsHash: hashes["permissionsHash"] ?? "",
            policyHash: hashes["policyHash"] ?? "",
            signedAt: signedAt
        )
        guard signingKey.publicKey.isValidSignature(signatureData, for: Data(payload.utf8)) else {
            return ("signature_invalid", "Ed25519 signature verification failed")
        }
        return nil
    }

    private func installedVersionForMount(
        appId: String,
        installId: String
    ) -> (version: String, runtimeVersion: String, dataVersion: Int, manifest: [String: Any], contentHash: String, signature: [String: Any]?, status: String)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = """
        SELECT version, runtime_version, data_version, manifest_json, content_hash, signature_json, status
        FROM app_versions
        WHERE app_id = ? AND install_id = ?
        LIMIT 1
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        let manifest = jsonDictionary(columnText(statement, 3)) ?? [:]
        let signature = jsonValue(columnNullableText(statement, 5)) as? [String: Any]
        return (
            version: columnText(statement, 0),
            runtimeVersion: columnText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2)),
            manifest: manifest,
            contentHash: columnText(statement, 4),
            signature: signature,
            status: columnText(statement, 6)
        )
    }

    private func installedFiles(installId: String) -> [(path: String, content: String)] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        let sql = "SELECT path, content_text FROM app_files WHERE install_id = ? ORDER BY path"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, installId)
        var files: [(path: String, content: String)] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            files.append((path: columnText(statement, 0), content: columnText(statement, 1)))
        }
        return files
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
        exportBackup(type: "debug-bundle", includeDebug: true)
    }

    private func exportBackup(type: String, includeDebug: Bool) -> [String: Any] {
        let exportId = "export_\(UUID().uuidString.lowercased())"
        let createdAt = Self.now()
        var document: [String: Any] = [
            "exportId": exportId,
            "type": type,
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
                columns: ["install_id", "app_id", "version", "runtime_version", "data_version", "manifest_json", "manifest_hash", "content_hash", "signature_json", "trust_level", "status", "created_at", "activated_at"],
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
            "appMigrations": tableRows(
                table: "app_migrations",
                columns: ["migration_id", "app_id", "from_data_version", "to_data_version", "migration_json", "content_hash", "created_at"],
                orderBy: "created_at"
            ),
            "runtimeCapabilities": runtimeCapabilities(appId: nil),
            "debug": includeDebug ? [
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
            ] : [:],
        ]
        let contentHash = "sha256:\(sha256Hex(jsonBody(document)))"
        document["contentHash"] = contentHash
        recordBackupExport(document, type: type, contentHash: contentHash, createdAt: createdAt)
        return document
    }

    private func importBackup(_ document: [String: Any]) -> [String: Any]? {
        guard ["backup", "debug-bundle", "test-fixture"].contains(document["type"] as? String ?? ""),
              document["apps"] is [[String: Any]],
              document["appVersions"] is [[String: Any]],
              document["appFiles"] is [[String: Any]],
              document["appPermissions"] is [[String: Any]],
              document["appStorage"] is [[String: Any]]
        else {
            return nil
        }
        let createdAt = Self.now()
        let apps = document["apps"] as? [[String: Any]] ?? []
        let versions = document["appVersions"] as? [[String: Any]] ?? []
        let files = document["appFiles"] as? [[String: Any]] ?? []
        let permissions = document["appPermissions"] as? [[String: Any]] ?? []
        let storageRows = document["appStorage"] as? [[String: Any]] ?? []
        let migrations = document["appMigrations"] as? [[String: Any]] ?? []
        let reports = document["appInstallReports"] as? [[String: Any]] ?? []

        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true

        for app in apps {
            guard let appId = textValue(app, keys: ["id", "appId"]) else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    appId,
                    textValue(app, keys: ["name"]) ?? appId,
                    textValue(app, keys: ["status"]) ?? "enabled",
                    textValue(app, keys: ["active_install_id", "activeInstallId"]),
                    textValue(app, keys: ["active_version", "activeVersion"]),
                    intValue(firstValue(app, keys: ["data_version", "dataVersion"])) ?? 1,
                    textValue(app, keys: ["created_at", "createdAt"]) ?? createdAt,
                    textValue(app, keys: ["updated_at", "updatedAt"]) ?? createdAt,
                ]
            )
        }

        for version in versions where ok {
            guard let installId = textValue(version, keys: ["install_id", "installId"]),
                  let appId = textValue(version, keys: ["app_id", "appId"]),
                  let appVersion = textValue(version, keys: ["version", "appVersion"])
            else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    installId,
                    appId,
                    appVersion,
                    textValue(version, keys: ["runtime_version", "runtimeVersion"]) ?? "0.1.0",
                    intValue(firstValue(version, keys: ["data_version", "dataVersion"])) ?? 1,
                    jsonTextValue(version, stringKeys: ["manifest_json", "manifestJson"], objectKeys: ["manifest"], fallback: "{}"),
                    textValue(version, keys: ["manifest_hash", "manifestHash"]) ?? "",
                    textValue(version, keys: ["content_hash", "contentHash"]) ?? "",
                    jsonTextValue(version, stringKeys: ["signature_json", "signatureJson"], objectKeys: ["signature"], fallback: nil),
                    textValue(version, keys: ["trust_level", "trustLevel"]) ?? "developer",
                    textValue(version, keys: ["status"]) ?? "installed",
                    textValue(version, keys: ["created_at", "installedAt", "createdAt"]) ?? createdAt,
                    textValue(version, keys: ["activated_at", "activatedAt"]),
                ]
            )
        }

        for file in files where ok {
            guard let installId = textValue(file, keys: ["install_id", "installId"]),
                  let path = textValue(file, keys: ["path"])
            else {
                ok = false
                break
            }
            let content = textValue(file, keys: ["content_text", "contentText"]) ?? ""
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    installId,
                    path,
                    content,
                    textValue(file, keys: ["content_hash", "contentHash"]) ?? "sha256:\(sha256Hex(content))",
                    intValue(firstValue(file, keys: ["size_bytes", "sizeBytes"])) ?? content.utf8.count,
                    textValue(file, keys: ["mime"]) ?? "text/plain",
                    textValue(file, keys: ["created_at", "createdAt"]) ?? createdAt,
                ]
            )
        }

        for permission in permissions where ok {
            guard let installId = textValue(permission, keys: ["install_id", "installId"]),
                  let appId = textValue(permission, keys: ["app_id", "appId"]),
                  let name = textValue(permission, keys: ["permission"])
            else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    installId,
                    appId,
                    name,
                    intValue(firstValue(permission, keys: ["requested"])) ?? 1,
                    intValue(firstValue(permission, keys: ["approved"])) ?? 0,
                    textValue(permission, keys: ["approved_at", "approvedAt"]),
                    textValue(permission, keys: ["reason"]) ?? "imported",
                ]
            )
        }

        for storage in storageRows where ok {
            guard let appId = textValue(storage, keys: ["app_id", "appId"]),
                  let key = textValue(storage, keys: ["key"])
            else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at)
                VALUES (?, ?, ?, ?)
                """,
                [
                    appId,
                    key,
                    jsonTextValue(storage, stringKeys: ["value_json", "valueJson"], objectKeys: ["value"], fallback: "null") ?? "null",
                    textValue(storage, keys: ["updated_at", "updatedAt"]) ?? createdAt,
                ]
            )
        }

        for migration in migrations where ok {
            guard let migrationId = textValue(migration, keys: ["migration_id", "migrationId"]),
                  let appId = textValue(migration, keys: ["app_id", "appId"])
            else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    migrationId,
                    appId,
                    intValue(firstValue(migration, keys: ["from_data_version", "fromDataVersion"])) ?? 1,
                    intValue(firstValue(migration, keys: ["to_data_version", "toDataVersion"])) ?? 1,
                    jsonTextValue(migration, stringKeys: ["migration_json", "migrationJson"], objectKeys: ["migration"], fallback: "{}"),
                    textValue(migration, keys: ["content_hash", "contentHash"]) ?? "",
                    textValue(migration, keys: ["created_at", "createdAt"]) ?? createdAt,
                ]
            )
        }

        for report in reports where ok {
            guard let reportId = textValue(report, keys: ["report_id", "reportId"]),
                  let appId = textValue(report, keys: ["app_id", "appId"])
            else {
                ok = false
                break
            }
            ok = ok && executePrepared(
                """
                INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    reportId,
                    appId,
                    textValue(report, keys: ["install_id", "installId"]),
                    textValue(report, keys: ["status"]) ?? "accepted",
                    jsonTextValue(report, stringKeys: ["validation_json", "validationJson"], objectKeys: ["validation"], fallback: nil),
                    jsonTextValue(report, stringKeys: ["security_json", "securityJson"], objectKeys: ["security"], fallback: nil),
                    jsonTextValue(report, stringKeys: ["permissions_json", "permissionsJson"], objectKeys: ["permissions"], fallback: nil),
                    jsonTextValue(report, stringKeys: ["compatibility_json", "compatibilityJson"], objectKeys: ["compatibility"], fallback: nil),
                    jsonTextValue(report, stringKeys: ["smoke_test_json", "smokeTestJson"], objectKeys: ["smokeTest"], fallback: nil),
                    textValue(report, keys: ["content_hash", "contentHash"]),
                    textValue(report, keys: ["created_at", "createdAt"]) ?? createdAt,
                ]
            )
        }

        let source = document["source"] as? [String: Any] ?? [:]
        ok = ok && executePrepared(
            """
            INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at)
            VALUES (?, 'import', ?, ?, ?, ?, ?, ?)
            """,
            [
                "import_\(UUID().uuidString.lowercased())",
                textValue(source, keys: ["platform"]) ?? "unknown",
                textValue(document, keys: ["runtimeVersion"]) ?? "0.4.0",
                jsonBody(document),
                textValue(document, keys: ["contentHash"]) ?? "sha256:\(sha256Hex(jsonBody(document)))",
                createdAt,
                createdAt,
            ]
        )

        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            return nil
        }
        return [
            "ok": true,
            "apps": apps.count,
            "appVersions": versions.count,
            "appStorage": storageRows.count,
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
            resourceBudget: AppSandboxContext.resourceBudget(from: manifest),
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
        } else if let budgetResponse = bridgeRateBudgetFailure(request) {
            response = budgetResponse
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
            return appLog(request)
        case "dialog.openFile", "dialog.saveFile":
            return mockedDialogResponse(request)
                ?? .failure(id: request.id, code: "platform_unsupported", message: "\(request.method) requires an interactive macOS dialog")
        default:
            return .failure(id: request.id, code: "unknown_method", message: "Unknown bridge method: \(request.method)")
        }
    }

    private func bridgeRateBudgetFailure(_ request: BridgeRequest) -> BridgeResponse? {
        if let limit = request.context.resourceBudget["maxBridgeCallsPerMinute"] {
            let current = bridgeCallCount(appId: request.context.appId, seconds: 60)
            if current >= limit {
                return .failure(
                    id: request.id,
                    code: "resource_budget_exceeded",
                    message: "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute",
                    details: [
                        "appId": request.context.appId,
                        "budget": "maxBridgeCallsPerMinute",
                        "current": current,
                        "max": limit,
                        "limit": limit
                    ]
                )
            }
        }
        if request.method == "network.request",
           let limit = request.context.resourceBudget["maxNetworkRequestsPerMinute"] {
            let current = bridgeCallCount(appId: request.context.appId, method: "network.request", seconds: 60)
            if current >= limit {
                return .failure(
                    id: request.id,
                    code: "resource_budget_exceeded",
                    message: "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute",
                    details: [
                        "appId": request.context.appId,
                        "budget": "maxNetworkRequestsPerMinute",
                        "current": current,
                        "max": limit,
                        "limit": limit
                    ]
                )
            }
        }
        return nil
    }

    private func appLog(_ request: BridgeRequest) -> BridgeResponse {
        guard let level = request.params["level"] as? String,
              ["debug", "info", "warn", "error"].contains(level)
        else {
            return .failure(id: request.id, code: "invalid_request", message: "app.log level must be debug, info, warn, or error")
        }
        guard let message = request.params["message"] as? String, !message.isEmpty else {
            return .failure(id: request.id, code: "invalid_request", message: "app.log requires message")
        }
        if let limit = request.context.resourceBudget["maxLogLinesPerMinute"] {
            let current = bridgeCallCount(appId: request.context.appId, method: "app.log", seconds: 60)
            if current >= limit {
                return .failure(
                    id: request.id,
                    code: "resource_budget_exceeded",
                    message: "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute",
                    details: [
                        "budget": "maxLogLinesPerMinute",
                        "current": current,
                        "max": limit,
                        "limit": limit
                    ]
                )
            }
        }
        NSLog("Generated app log [\(level)]: \(message)")
        return .success(id: request.id, result: ["ok": true])
    }

    private func bridgeCallCount(appId: String, seconds: Int) -> Int {
        scalarInt(
            "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND datetime(created_at) >= datetime('now', ?)",
            values: [appId, "-\(seconds) seconds"]
        )
    }

    private func bridgeCallCount(appId: String, method: String, seconds: Int) -> Int {
        scalarInt(
            "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND datetime(created_at) >= datetime('now', ?)",
            values: [appId, method, "-\(seconds) seconds"]
        )
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
        guard let rule = request.context.networkPolicy.first(where: { $0.allows(origin: origin, method: method, path: PlatformNetwork.path(for: url), headers: Array(headers.keys)) }) else {
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
        let requestedTimeout = mockedNetworkRequestTimeoutMs(request.params)
        if let invalidValue = requestedTimeout.invalidValue {
            return .failure(
                id: request.id,
                code: "invalid_request",
                message: "network.request timeoutMs must be a positive integer",
                details: ["timeoutMs": invalidValue]
            )
        }
        if let delayMs = mockedNetworkDelayMs(mock) {
            let effectiveTimeoutMs = effectiveMockedNetworkTimeoutMs(rule: rule, requestedTimeoutMs: requestedTimeout.value)
            if delayMs > effectiveTimeoutMs {
                return .failure(
                    id: request.id,
                    code: "timeout",
                    message: "network.request timed out",
                    details: ["timeoutMs": effectiveTimeoutMs, "delayMs": delayMs]
                )
            }
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
        let sql = """
        INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        do {
            var statement: OpaquePointer?
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
            guard sqlite3_step(statement) == SQLITE_DONE else { return }
        }
        BridgeBudgetQuarantine.maybeQuarantineAfterBudgetError(
            database: database.handle,
            appId: appId,
            installId: active?.installId,
            error: response.error,
            actor: "macos-control-runtime"
        )
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

    private func recordTestRun(
        microTestId: String,
        name: String,
        appId: String?,
        spec: Any,
        status: String,
        result: [String: Any],
        diagnostics: [String: Any]
    ) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        let startedAt = Self.now()
        let finishedAt = Self.now()
        let testRunId = "testrun_\(UUID().uuidString.lowercased())"
        let sessionId = appId.map { runtimeSessionId(appId: $0) }

        var microStatement: OpaquePointer?
        let microSQL = """
        INSERT INTO micro_tests (micro_test_id, app_id, name, spec_json, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(micro_test_id) DO UPDATE SET
          app_id = excluded.app_id,
          name = excluded.name,
          spec_json = excluded.spec_json,
          updated_at = excluded.updated_at
        """
        guard sqlite3_prepare_v2(db, microSQL, -1, &microStatement, nil) == SQLITE_OK else {
            return nil
        }
        bind(microStatement, 1, microTestId)
        bindNullable(microStatement, 2, appId)
        bind(microStatement, 3, name)
        bind(microStatement, 4, jsonString(spec))
        bind(microStatement, 5, startedAt)
        bind(microStatement, 6, startedAt)
        guard sqlite3_step(microStatement) == SQLITE_DONE else {
            sqlite3_finalize(microStatement)
            return nil
        }
        sqlite3_finalize(microStatement)

        var runStatement: OpaquePointer?
        let runSQL = """
        INSERT INTO test_runs (test_run_id, micro_test_id, session_id, control_session_id, app_id, status, started_at, finished_at, result_json, diagnostics_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, runSQL, -1, &runStatement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(runStatement) }
        bind(runStatement, 1, testRunId)
        bind(runStatement, 2, microTestId)
        bindNullable(runStatement, 3, sessionId)
        bind(runStatement, 4, controlSessionId)
        bindNullable(runStatement, 5, appId)
        bind(runStatement, 6, status)
        bind(runStatement, 7, startedAt)
        bind(runStatement, 8, finishedAt)
        bind(runStatement, 9, jsonBody(result))
        bind(runStatement, 10, jsonBody(diagnostics))
        guard sqlite3_step(runStatement) == SQLITE_DONE else {
            return nil
        }
        return [
            "testRunId": testRunId,
            "microTestId": microTestId,
            "appId": appId ?? NSNull(),
            "status": status,
            "result": result,
        ]
    }

    private func microtestSpec(args: [String: Any]) -> [String: Any]? {
        if let spec = args["spec"] as? [String: Any] {
            return spec
        }
        guard let path = args["microtestPath"] as? String ?? args["path"] as? String,
              let url = repoRelativeURL(path)
        else {
            return nil
        }
        guard let text = try? String(contentsOf: url, encoding: .utf8),
              let data = text.data(using: .utf8),
              let spec = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        return spec
    }

    private func platformSmokeSpec(args: [String: Any]) -> [String: Any]? {
        if let spec = args["spec"] as? [String: Any] {
            return spec
        }
        guard let path = args["smokePath"] as? String ?? args["path"] as? String,
              let url = repoRelativeURL(path)
        else {
            return nil
        }
        guard let text = try? String(contentsOf: url, encoding: .utf8),
              let data = text.data(using: .utf8),
              let spec = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        return spec
    }

    private func validatePackageResult(args: [String: Any]) -> [String: Any]? {
        guard let package = readPackage(args: args) else {
            return nil
        }
        let ok = package.errors.isEmpty
        return [
            "ok": ok,
            "appId": package.manifest["id"] ?? NSNull(),
            "version": package.manifest["version"] ?? NSNull(),
            "runtimeVersion": package.manifest["runtimeVersion"] ?? NSNull(),
            "dataVersion": package.manifest["dataVersion"] ?? NSNull(),
            "files": package.files.map { $0.path }.sorted(),
            "bridgeMethods": bridgeMethods(in: package.files.first { $0.path == "app.js" }?.content ?? "").sorted(),
            "errors": package.errors,
            "warnings": package.warnings,
        ]
    }

    private func signPackageResult(args: [String: Any]) -> [String: Any]? {
        guard let package = readPackage(args: args) else {
            return nil
        }
        let hashes = packageHashes(package)
        let trustLevel = args["trustLevel"] as? String ?? package.manifest["trust"].flatMap { ($0 as? [String: Any])?["level"] as? String } ?? "developer"
        let appId = package.manifest["id"] as? String ?? ""
        let appVersion = package.manifest["version"] as? String ?? ""
        let runtimeVersion = package.manifest["runtimeVersion"] as? String ?? ""
        let dataVersion = intValue(package.manifest["dataVersion"]) ?? 1
        let signedAt = Self.now()
        let keyId = signingKeyId()
        var signature: [String: Any] = [
            "algorithm": "ed25519",
            "keyId": keyId,
            "appId": appId,
            "appVersion": appVersion,
            "runtimeVersion": runtimeVersion,
            "dataVersion": dataVersion,
            "manifestHash": hashes["manifestHash"] ?? "",
            "contentHash": hashes["contentHash"] ?? "",
            "permissionsHash": hashes["permissionsHash"] ?? "",
            "policyHash": hashes["policyHash"] ?? "",
            "trustLevel": trustLevel,
            "signedAt": signedAt,
            "signedBy": "macos-dev-control",
        ]
        signature["signature"] = signPayload(signaturePayload(
            appId: appId,
            appVersion: appVersion,
            dataVersion: dataVersion,
            runtimeVersion: runtimeVersion,
            trustLevel: trustLevel,
            keyId: keyId,
            manifestHash: hashes["manifestHash"] ?? "",
            contentHash: hashes["contentHash"] ?? "",
            permissionsHash: hashes["permissionsHash"] ?? "",
            policyHash: hashes["policyHash"] ?? "",
            signedAt: signedAt
        ))
        return [
            "ok": package.errors.isEmpty,
            "appId": package.manifest["id"] ?? NSNull(),
            "version": package.manifest["version"] ?? NSNull(),
            "keyId": keyId,
            "signature": signature,
            "hashes": hashes,
            "errors": package.errors,
            "warnings": package.warnings,
        ]
    }

    private func installPackageResult(args: [String: Any]) -> [String: Any]? {
        guard let package = readPackage(args: args) else {
            return nil
        }
        guard package.errors.isEmpty else {
            return [
                "ok": false,
                "status": "failed",
                "appId": package.manifest["id"] ?? NSNull(),
                "errors": package.errors,
                "warnings": package.warnings,
            ]
        }
        let smoke = smokeResult(package: package)
        let accessibility = accessibilityAuditForHTML(
            appId: package.manifest["id"] as? String ?? "",
            html: package.files.first { $0.path == "index.html" }?.content ?? ""
        )
        let compatibility = runtimeCompatibilityResult(package.manifest["runtimeVersion"] as? String)
        let ok = (smoke["ok"] as? Bool) == true && (accessibility["status"] as? String) != "fail" && (compatibility["ok"] as? Bool) == true
        let approval = updateApproval(for: package.manifest)
        let reportStatus = ok ? ((approval["requiresUserApproval"] as? Bool) == true ? "requires-approval" : "accepted") : "failed"
        guard let install = insertInstalledPackage(package: package, smoke: smoke, accessibility: accessibility, compatibility: compatibility, reportStatus: reportStatus, approval: approval) else {
            return [
                "ok": false,
                "status": "failed",
                "appId": package.manifest["id"] ?? NSNull(),
                "errors": [["code": "sqlite_error", "message": "Package install transaction failed"]],
                "warnings": package.warnings,
            ]
        }
        _ = recordTestRun(
            microTestId: "smoke:\(install["appId"] as? String ?? "")",
            name: "\(install["appId"] as? String ?? "") bundled smoke tests",
            appId: install["appId"] as? String,
            spec: smoke["spec"] ?? [],
            status: (smoke["ok"] as? Bool) == true ? "passed" : "failed",
            result: smoke,
            diagnostics: ["runner": "native-macos-static-install"]
        )
        return [
            "ok": ok,
            "status": ok ? (((approval["requiresUserApproval"] as? Bool) == true) ? "requires-approval" : "enabled") : "quarantined",
            "installId": install["installId"] ?? NSNull(),
            "reportId": install["reportId"] ?? NSNull(),
            "appId": install["appId"] ?? NSNull(),
            "version": install["version"] ?? NSNull(),
            "contentHash": install["contentHash"] ?? NSNull(),
            "approval": approval,
            "smokeTest": smoke,
            "accessibility": accessibility,
            "compatibility": compatibility,
            "warnings": package.warnings,
        ]
    }

    private func readPackage(args: [String: Any]) -> PackageRead? {
        guard let path = args["packagePath"] as? String ?? args["path"] as? String,
              let directory = repoRelativeURL(path)
        else {
            return nil
        }
        return readPackage(directory: directory)
    }

    private func readPackage(directory: URL) -> PackageRead {
        let required = ["manifest.json", "index.html", "styles.css", "app.js"]
        let optional = Set(["smoke-tests.json", "README.md"])
        var errors: [[String: Any]] = []
        var warnings: [[String: Any]] = []
        var files: [PackageFile] = []

        guard let enumerator = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: [.isRegularFileKey]) else {
            return PackageRead(directory: directory, manifest: [:], manifestJSON: "{}", files: [], errors: [
                packageIssue("package_not_found", "Package directory was not found", details: ["path": directory.path]),
            ], warnings: [])
        }

        for case let fileURL as URL in enumerator {
            let values = try? fileURL.resourceValues(forKeys: [.isRegularFileKey])
            guard values?.isRegularFile == true else { continue }
            let relativePath = fileURL.path.replacingOccurrences(of: directory.path + "/", with: "")
            if relativePath.hasPrefix("assets/") || (!required.contains(relativePath) && !optional.contains(relativePath) && !relativePath.hasPrefix("migrations/")) {
                errors.append(packageIssue("unexpected_package_path", "Package contains an unexpected path", details: ["path": relativePath]))
                continue
            }
            let content = (try? String(contentsOf: fileURL, encoding: .utf8)) ?? ""
            files.append(PackageFile(
                path: relativePath,
                content: content,
                contentHash: "sha256:\(sha256Hex(content))",
                sizeBytes: content.utf8.count,
                mime: mimeType(forPackagePath: relativePath)
            ))
        }

        if files.count > 32 {
            errors.append(packageIssue("resource_budget_exceeded", "Package exceeds hard file count cap", details: ["files": files.count, "maxFiles": 32]))
        }
        let migrationCount = files.filter { $0.path.hasPrefix("migrations/") }.count
        if migrationCount > 16 {
            errors.append(packageIssue("resource_budget_exceeded", "Package exceeds hard migration file count cap", details: ["migrations": migrationCount, "maxMigrations": 16]))
        }
        for path in required where !files.contains(where: { $0.path == path }) {
            errors.append(packageIssue("missing_required_file", "\(path) is required", details: ["path": path]))
        }

        let manifestJSON = files.first { $0.path == "manifest.json" }?.content ?? "{}"
        let manifest = jsonDictionary(manifestJSON) ?? [:]
        if manifest.isEmpty {
            errors.append(packageIssue("invalid_manifest_json", "manifest.json must parse as JSON", details: [:]))
        } else {
            validatePackageManifest(manifest, errors: &errors)
            validatePackageBudgets(manifest, files: files, errors: &errors)
            validatePackageBridgePermissions(manifest, appJs: files.first { $0.path == "app.js" }?.content ?? "", errors: &errors)
        }
        validateGeneratedSourcePolicy(files.first { $0.path == "app.js" }?.content ?? "", errors: &errors)

        if !files.contains(where: { $0.path == "smoke-tests.json" }) {
            warnings.append(packageIssue("smoke_tests_missing", "Package has no smoke-tests.json", details: [:]))
        }

        return PackageRead(
            directory: directory,
            manifest: manifest,
            manifestJSON: manifestJSON,
            files: files.sorted { $0.path < $1.path },
            errors: errors,
            warnings: warnings
        )
    }

    private func validatePackageManifest(_ manifest: [String: Any], errors: inout [[String: Any]]) {
        for field in ["id", "name", "version", "runtimeVersion", "entry", "description", "permissions", "storagePrefix", "dataVersion", "capabilities", "resourceBudget", "networkPolicy"] where manifest[field] == nil {
            errors.append(packageIssue("missing_manifest_field", "manifest.\(field) is required", details: ["field": field]))
        }
        if manifest["networkAllowlist"] != nil {
            errors.append(packageIssue("removed_manifest_field", "manifest.networkAllowlist was removed; use networkPolicy", details: ["field": "networkAllowlist"]))
        }
        if let id = manifest["id"] as? String {
            if id.range(of: #"^[a-z][a-z0-9-]{2,63}$"#, options: .regularExpression) == nil {
                errors.append(packageIssue("invalid_manifest_id", "manifest.id must be lowercase kebab-case", details: ["value": id]))
            }
            if (manifest["storagePrefix"] as? String) != "\(id):" {
                errors.append(packageIssue("invalid_storage_prefix", "manifest.storagePrefix must equal <id>:", details: ["expected": "\(id):", "actual": manifest["storagePrefix"] ?? NSNull()]))
            }
        }
        if manifest["entry"] as? String != "index.html" {
            errors.append(packageIssue("invalid_entry", "manifest.entry must be index.html", details: ["value": manifest["entry"] ?? NSNull()]))
        }
        if (intValue(manifest["dataVersion"]) ?? 0) < 1 {
            errors.append(packageIssue("invalid_data_version", "manifest.dataVersion must be a positive integer", details: ["value": manifest["dataVersion"] ?? NSNull()]))
        }
        if !(manifest["permissions"] is [String]) {
            errors.append(packageIssue("invalid_permissions", "manifest.permissions must be an array", details: [:]))
        }
        if !(manifest["capabilities"] is [String: Any]) {
            errors.append(packageIssue("invalid_capabilities", "manifest.capabilities is required", details: [:]))
        }
        if !(manifest["resourceBudget"] is [String: Any]) {
            errors.append(packageIssue("invalid_resource_budget", "manifest.resourceBudget must be an object", details: [:]))
        }
        if !(manifest["networkPolicy"] is [String: Any]) {
            errors.append(packageIssue("invalid_network_policy", "manifest.networkPolicy must be an object", details: [:]))
        }
    }

    private func validatePackageBudgets(_ manifest: [String: Any], files: [PackageFile], errors: inout [[String: Any]]) {
        let budget = manifest["resourceBudget"] as? [String: Any] ?? [:]
        let maxPackageBytes = intValue(budget["maxPackageBytes"]) ?? 1_048_576
        let maxFileBytes = intValue(budget["maxFileBytes"]) ?? 524_288
        let totalBytes = files.reduce(0) { $0 + $1.sizeBytes }
        if totalBytes > maxPackageBytes {
            errors.append(packageIssue("resource_budget_exceeded", "Package exceeds manifest.resourceBudget.maxPackageBytes", details: ["bytes": totalBytes, "maxPackageBytes": maxPackageBytes]))
        }
        for file in files where file.sizeBytes > maxFileBytes {
            errors.append(packageIssue("resource_budget_exceeded", "Package file exceeds manifest.resourceBudget.maxFileBytes", details: ["path": file.path, "bytes": file.sizeBytes, "maxFileBytes": maxFileBytes]))
        }
    }

    private func validatePackageBridgePermissions(_ manifest: [String: Any], appJs: String, errors: inout [[String: Any]]) {
        let permissions = Set(manifest["permissions"] as? [String] ?? [])
        for method in bridgeMethods(in: appJs) {
            guard let permission = permissionForBridgeMethod(method), !permissions.contains(permission) else {
                continue
            }
            errors.append(packageIssue("missing_permission", "manifest.permissions does not cover a bridge method used by app.js", details: ["method": method, "permission": permission]))
        }
    }

    private func validateGeneratedSourcePolicy(_ appJs: String, errors: inout [[String: Any]]) {
        let checks: [(String, String)] = [
            ("forbidden_eval", #"\beval\s*\("#),
            ("forbidden_function_constructor", #"\bnew\s+Function\s*\("#),
            ("forbidden_dynamic_import", #"\bimport\s*\("#),
            ("forbidden_network_api", #"\bfetch\s*\("#),
            ("forbidden_network_api", #"\bXMLHttpRequest\b"#),
            ("forbidden_storage_api", #"\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bdocument\.cookie\b"#),
            ("forbidden_native_bridge", #"\bwebkit\.messageHandlers\b|\bchrome\.webview\b|\bAndroid\.|\bNativeAIPlatformBridge\b"#),
        ]
        for (code, pattern) in checks where appJs.range(of: pattern, options: .regularExpression) != nil {
            errors.append(packageIssue(code, "app.js uses a forbidden generated-app API", details: [:]))
        }
    }

    private func insertInstalledPackage(
        package: PackageRead,
        smoke: [String: Any],
        accessibility: [String: Any],
        compatibility: [String: Any],
        reportStatus: String,
        approval: [String: Any]
    ) -> [String: Any]? {
        guard database.handle != nil,
              let appId = package.manifest["id"] as? String,
              let name = package.manifest["name"] as? String,
              let version = package.manifest["version"] as? String,
              let runtimeVersion = package.manifest["runtimeVersion"] as? String
        else {
            return nil
        }
        let dataVersion = intValue(package.manifest["dataVersion"]) ?? 1
        let hashes = packageHashes(package)
        let now = Self.now()
        let installId = "install_\(appId)_\(version.replacingOccurrences(of: ".", with: "_"))_\(UUID().uuidString.lowercased())"
        let reportId = "report_\(UUID().uuidString.lowercased())"
        let previous = appRecord(appId: appId)
        let previousInstallId = previous?.activeInstallId
        let signature = signPackageResult(args: ["path": package.directory.path])?["signature"] ?? NSNull()
        let shouldActivate = reportStatus == "accepted"
        let status = shouldActivate ? "enabled" : (reportStatus == "requires-approval" ? "installed" : "quarantined")
        let appStatus = shouldActivate || previousInstallId != nil ? "enabled" : (status == "quarantined" ? "quarantined" : "disabled")
        let appDataVersion = shouldActivate || previous == nil ? dataVersion : previous?.dataVersion ?? dataVersion
        let approvedPermissions = shouldActivate ? (package.manifest["permissions"] as? [String] ?? []) : []

        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true
        ok = ok && executePrepared(
            """
            INSERT INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at)
            VALUES (?, ?, ?, NULL, NULL, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status, data_version = excluded.data_version, updated_at = excluded.updated_at
            """,
            [appId, name, appStatus, appDataVersion, now, now]
        )
        if let previousInstallId, shouldActivate {
            ok = ok && executePrepared("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", [previousInstallId])
        }
        ok = ok && executePrepared(
            """
            INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'developer', ?, ?, ?)
            """,
            [installId, appId, version, runtimeVersion, dataVersion, package.manifestJSON, hashes["manifestHash"] ?? "", hashes["contentHash"] ?? "", jsonString(signature), status, now, status == "enabled" ? now : nil]
        )
        for file in package.files {
            ok = ok && executePrepared(
                """
                INSERT INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                [installId, file.path, file.content, file.contentHash, file.sizeBytes, file.mime, now]
            )
        }
        for permission in package.manifest["permissions"] as? [String] ?? [] {
            ok = ok && executePrepared(
                """
                INSERT INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason)
                VALUES (?, ?, ?, 1, ?, ?, 'dev-control install')
                """,
                [installId, appId, permission, shouldActivate ? 1 : 0, shouldActivate ? now : nil]
            )
        }
        var permissionsReport = approval
        permissionsReport["approved"] = approvedPermissions
        permissionsReport["requested"] = package.manifest["permissions"] as? [String] ?? []
        ok = ok && executePrepared(
            """
            INSERT INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                reportId,
                appId,
                installId,
                reportStatus,
                jsonBody(["ok": true, "errors": package.errors, "warnings": package.warnings]),
                jsonBody(["ok": true, "signature": signature, "accessibility": accessibility]),
                jsonBody(permissionsReport),
                jsonBody(compatibility),
                jsonBody(smoke),
                hashes["contentHash"] ?? "",
                now,
            ]
        )
        ok = ok && executePrepared(
            """
            INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json)
            VALUES (?, ?, ?, 'install', ?, 'codex', ?, ?, ?)
            """,
            ["event_\(UUID().uuidString.lowercased())", appId, installId, previousInstallId, reportId, now, jsonBody(["source": "macos-dev-control", "status": status] as [String: Any])]
        )
        if shouldActivate {
            ok = ok && executePrepared(
                "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, updated_at = ? WHERE id = ?",
                [installId, version, dataVersion, now, appId]
            )
            ok = ok && executePrepared(
                """
                INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json)
                VALUES (?, ?, ?, 'activate', ?, 'codex', ?, ?, ?)
                """,
                ["event_\(UUID().uuidString.lowercased())", appId, installId, previousInstallId, reportId, now, jsonBody(["source": "macos-dev-control"] as [String: Any])]
            )
        } else if status == "quarantined" {
            ok = ok && executePrepared(
                """
                INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json)
                VALUES (?, ?, ?, 'quarantine', ?, 'codex', ?, ?, ?)
                """,
                ["event_\(UUID().uuidString.lowercased())", appId, installId, previousInstallId, reportId, now, jsonBody(["reason": "install gate failed"] as [String: Any])]
            )
        }

        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            recordInstallStorageFailureReport(
                appId: appId,
                name: name,
                installId: installId,
                reportId: reportId,
                dataVersion: dataVersion,
                contentHash: hashes["contentHash"] ?? "",
                createdAt: now
            )
            return nil
        }
        return [
            "installId": installId,
            "reportId": reportId,
            "appId": appId,
            "version": version,
            "contentHash": hashes["contentHash"] ?? "",
        ]
    }

    private func recordInstallStorageFailureReport(
        appId: String,
        name: String,
        installId: String,
        reportId: String,
        dataVersion: Int,
        contentHash: String,
        createdAt: String
    ) {
        guard executeSQL("BEGIN IMMEDIATE") else { return }
        var ok = true
        ok = ok && executePrepared(
            """
            INSERT INTO apps (id, name, status, data_version, created_at, updated_at)
            VALUES (?, ?, 'disabled', ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at
            """,
            [appId, name, dataVersion, createdAt, createdAt]
        )
        ok = ok && executePrepared(
            """
            INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at)
            VALUES (?, ?, NULL, 'failed', ?, '{}', '{}', '{}', '{}', ?, ?)
            """,
            [
                reportId,
                appId,
                jsonBody([
                    "ok": false,
                    "errors": [
                        [
                            "code": "storage_error",
                            "message": "Package install transaction failed while writing platform storage",
                            "details": ["installId": installId],
                        ],
                    ],
                ] as [String: Any]),
                contentHash,
                createdAt,
            ]
        )
        if ok, executeSQL("COMMIT") {
            return
        }
        _ = executeSQL("ROLLBACK")
    }

    private func updateApproval(for manifest: [String: Any]) -> [String: Any] {
        guard let appId = manifest["id"] as? String,
              let active = activeManifest(appId: appId)
        else {
            return ["requiresUserApproval": false, "reasons": []]
        }
        let checks: [(String, Any, Any)] = [
            ("permissions", sortedStringArray(active["permissions"]), sortedStringArray(manifest["permissions"])),
            ("networkPolicy", active["networkPolicy"] ?? [:], manifest["networkPolicy"] ?? [:]),
            ("resourceBudget", active["resourceBudget"] ?? [:], manifest["resourceBudget"] ?? [:]),
            ("capabilities", active["capabilities"] ?? [:], manifest["capabilities"] ?? [:]),
            ("dataVersion", active["dataVersion"] ?? NSNull(), manifest["dataVersion"] ?? NSNull()),
        ]
        let reasons = checks.compactMap { field, before, after in
            canonicalJSONEqual(before, after) ? nil : field
        }
        return [
            "requiresUserApproval": !reasons.isEmpty,
            "reasons": reasons,
            "approvalReasons": reasons,
            "previousInstallId": activeAppRecord(appId: appId)?.installId ?? NSNull(),
        ]
    }

    private func runPlatformSmoke(smoke: [String: Any], platform: String) -> [String: Any] {
        let apps = smoke["apps"] as? [String] ?? []
        let stepsPerApp = smoke["stepsPerApp"] as? [[String: Any]] ?? []
        var appResults: [[String: Any]] = []
        var failures: [[String: Any]] = []

        for appId in apps {
            var commands: [[String: Any]] = []
            let installStep: [String: Any] = [
                "tool": "platform.install_webapp_package",
                "args": ["path": "webapps/examples/\(appId)"],
            ]
            let install = executeMicrotestStep(phase: "setup", index: 0, step: installStep, appId: appId)
            commands.append(install)
            if (install["status"] as? String) == "failed" {
                var failure = install
                failure["appId"] = appId
                failures.append(failure)
            }

            for (index, step) in stepsPerApp.enumerated() {
                let expanded = expandPlatformSmokeStep(step, values: ["appId": appId, "platform": platform])
                let execution = executeMicrotestStep(phase: "steps", index: index, step: expanded, appId: appId)
                commands.append(execution)
                if (execution["status"] as? String) == "failed" {
                    var failure = execution
                    failure["appId"] = appId
                    failures.append(failure)
                }
            }

            appResults.append([
                "appId": appId,
                "ok": !commands.contains { ($0["status"] as? String) == "failed" },
                "commands": commands,
            ])
        }

        return [
            "ok": failures.isEmpty,
            "id": smoke["id"] ?? NSNull(),
            "platform": platform,
            "totalApps": apps.count,
            "failures": failures,
            "apps": appResults,
        ]
    }

    private func runMigration(migration: [String: Any], mode: String) throws -> [String: Any] {
        guard mode == "dry-run" || mode == "apply" else {
            throw NSError(domain: "DevControlPlane", code: 1)
        }
        guard let appId = migration["appId"] as? String,
              let fromDataVersion = intValue(migration["fromDataVersion"]),
              let toDataVersion = intValue(migration["toDataVersion"]),
              toDataVersion == fromDataVersion + 1
        else {
            throw NSError(domain: "DevControlPlane", code: 2)
        }
        guard let active = activeAppRecord(appId: appId), active.installId != nil else {
            throw NSError(domain: "DevControlPlane", code: 3)
        }
        guard active.dataVersion == fromDataVersion else {
            throw NSError(domain: "DevControlPlane", code: 4)
        }

        let startedAt = Self.now()
        let migrationId = "migration_\(appId)_\(fromDataVersion)_to_\(toDataVersion)"
        let runId = "mrun_\(UUID().uuidString.lowercased())"
        let preSnapshot = createSnapshot(appId: appId, type: "pre-migration", sessionId: runtimeSessionId(appId: appId))
        let preview = try previewMigration(migration: migration)
        let report: [String: Any] = [
            "changedKeys": preview["changedKeys"] ?? [],
            "operationCounts": preview["operationCounts"] ?? [:],
        ]

        guard executePrepared(
            """
            INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            [
                migrationId,
                appId,
                fromDataVersion,
                toDataVersion,
                jsonBody(migration),
                "sha256:\(sha256Hex(jsonString(migration)))",
                startedAt,
            ]
        ) else {
            throw NSError(domain: "DevControlPlane", code: 5)
        }

        if mode == "dry-run" {
            guard recordMigrationRun(
                runId: runId,
                migrationId: migrationId,
                appId: appId,
                installId: active.installId,
                mode: mode,
                status: "passed",
                preSnapshotId: preSnapshot?["snapshotId"] as? String,
                report: report,
                startedAt: startedAt
            ) else {
                throw NSError(domain: "DevControlPlane", code: 6)
            }
            return migrationRunResult(runId: runId, mode: mode, status: "passed", snapshotId: preSnapshot?["snapshotId"] as? String, preview: preview)
        }

        guard executeSQL("BEGIN IMMEDIATE") else {
            throw NSError(domain: "DevControlPlane", code: 7)
        }
        var ok = true
        for change in preview["changes"] as? [[String: Any]] ?? [] {
            guard let key = change["key"] as? String else {
                ok = false
                break
            }
            if change["delete"] as? Bool == true {
                ok = ok && executePrepared("DELETE FROM app_storage WHERE app_id = ? AND key = ?", [appId, key])
            } else {
                ok = ok && executePrepared(
                    """
                    INSERT INTO app_storage (app_id, key, value_json, updated_at)
                    VALUES (?, ?, ?, ?)
                    ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at
                    """,
                    [appId, key, jsonString(change["value"] ?? NSNull()), Self.now()]
                )
            }
        }
        ok = ok && executePrepared("UPDATE apps SET data_version = ?, updated_at = ? WHERE id = ?", [toDataVersion, Self.now(), appId])
        ok = ok && recordMigrationRun(
            runId: runId,
            migrationId: migrationId,
            appId: appId,
            installId: active.installId,
            mode: mode,
            status: "passed",
            preSnapshotId: preSnapshot?["snapshotId"] as? String,
            report: report,
            startedAt: startedAt
        )

        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            throw NSError(domain: "DevControlPlane", code: 8)
        }
        return migrationRunResult(runId: runId, mode: mode, status: "passed", snapshotId: preSnapshot?["snapshotId"] as? String, preview: preview)
    }

    private func previewMigration(migration: [String: Any]) throws -> [String: Any] {
        guard let appId = migration["appId"] as? String else {
            throw NSError(domain: "DevControlPlane", code: 9)
        }
        var values = storageValues(appId: appId)
        var changes: [[String: Any]] = []
        var operationCounts: [String: Int] = [:]
        for step in migration["steps"] as? [[String: Any]] ?? [] {
            guard let op = step["op"] as? String else {
                throw NSError(domain: "DevControlPlane", code: 10)
            }
            operationCounts[op] = (operationCounts[op] ?? 0) + 1
            switch op {
            case "setDefault":
                let keys = migrationKeys(step: step, values: values)
                let path = step["to"] as? String ?? step["jsonPath"] as? String ?? "$"
                for key in keys {
                    let next = setDefaultValue(values[key] ?? NSNull(), path: path, value: step["value"] ?? NSNull())
                    values[key] = next
                    changes.append(["key": key, "value": next])
                }
            case "renameKey", "moveStorageKey":
                guard let from = step["from"] as? String, let to = step["to"] as? String else {
                    throw NSError(domain: "DevControlPlane", code: 11)
                }
                let value = values[from] ?? NSNull()
                values.removeValue(forKey: from)
                values[to] = value
                changes.append(["key": from, "delete": true])
                changes.append(["key": to, "value": value])
            case "deleteKey", "deleteStorageKey":
                guard let key = step["key"] as? String else {
                    throw NSError(domain: "DevControlPlane", code: 12)
                }
                values.removeValue(forKey: key)
                changes.append(["key": key, "delete": true])
            case "copyKey":
                guard let from = step["from"] as? String, let to = step["to"] as? String else {
                    throw NSError(domain: "DevControlPlane", code: 13)
                }
                let value = values[from] ?? NSNull()
                values[to] = value
                changes.append(["key": to, "value": value])
            case "transformEnum":
                let keys = migrationKeys(step: step, values: values)
                let path = step["to"] as? String ?? step["jsonPath"] as? String ?? "$"
                let mapping = step["mapping"] as? [String: Any] ?? step["map"] as? [String: Any] ?? [:]
                for key in keys {
                    let next = transformEnumValue(values[key] ?? NSNull(), path: path, mapping: mapping, defaultValue: step["defaultMapping"])
                    values[key] = next
                    changes.append(["key": key, "value": next])
                }
            default:
                throw NSError(domain: "DevControlPlane", code: 14)
            }
        }
        let changedKeys = Array(Set(changes.compactMap { $0["key"] as? String })).sorted()
        return [
            "changedKeys": changedKeys,
            "operationCounts": operationCounts,
            "changes": changes,
        ]
    }

    private func recordMigrationRun(
        runId: String,
        migrationId: String,
        appId: String,
        installId: String?,
        mode: String,
        status: String,
        preSnapshotId: String?,
        report: [String: Any],
        startedAt: String
    ) -> Bool {
        executePrepared(
            """
            INSERT INTO migration_runs (migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, report_json, started_at, finished_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [runId, migrationId, appId, installId, mode, status, preSnapshotId, jsonBody(report), startedAt, Self.now()]
        )
    }

    private func migrationRunResult(runId: String, mode: String, status: String, snapshotId: String?, preview: [String: Any]) -> [String: Any] {
        var result = preview
        result["runId"] = runId
        result["mode"] = mode
        result["status"] = status
        result["snapshotId"] = snapshotId ?? NSNull()
        return result
    }

    private func storageValues(appId: String) -> [String: Any] {
        guard let db = database.handle else { return [:] }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT key, value_json FROM app_storage WHERE app_id = ? ORDER BY key", -1, &statement, nil) == SQLITE_OK else {
            return [:]
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        var values: [String: Any] = [:]
        while sqlite3_step(statement) == SQLITE_ROW {
            values[columnText(statement, 0)] = jsonValue(columnNullableText(statement, 1))
        }
        return values
    }

    private func migrationKeys(step: [String: Any], values: [String: Any]) -> [String] {
        if let key = step["key"] as? String {
            return [key]
        }
        guard let pattern = step["keyPattern"] as? String else {
            return []
        }
        if pattern.contains("*") || pattern.contains("?") {
            let regex = "^" + NSRegularExpression.escapedPattern(for: pattern)
                .replacingOccurrences(of: "\\*", with: ".*")
                .replacingOccurrences(of: "\\?", with: ".") + "$"
            return values.keys.filter { $0.range(of: regex, options: .regularExpression) != nil }.sorted()
        }
        return values.keys.filter { $0 == pattern }.sorted()
    }

    private func setDefaultValue(_ source: Any, path: String, value: Any) -> Any {
        var root = normalizedJSONObject(source)
        let components = migrationPathComponents(path)
        guard !components.isEmpty else {
            return source is NSNull ? value : source
        }
        setDefaultInObject(&root, components: components, value: value)
        return root
    }

    private func transformEnumValue(_ source: Any, path: String, mapping: [String: Any], defaultValue: Any?) -> Any {
        var root = normalizedJSONObject(source)
        let components = migrationPathComponents(path)
        guard !components.isEmpty else {
            let key = String(describing: source)
            return mapping[key] ?? defaultValue ?? source
        }
        transformEnumInObject(&root, components: components, mapping: mapping, defaultValue: defaultValue)
        return root
    }

    private func normalizedJSONObject(_ source: Any) -> Any {
        if source is NSNull {
            return [:] as [String: Any]
        }
        return source
    }

    private func migrationPathComponents(_ path: String) -> [String] {
        var value = path
        if value.hasPrefix("$.") {
            value.removeFirst(2)
        } else if value == "$" {
            return []
        }
        return value.split(separator: ".").map(String.init).filter { !$0.isEmpty && $0 != "*" && $0 != "[*]" }
    }

    private func setDefaultInObject(_ object: inout Any, components: [String], value: Any) {
        guard let first = components.first else { return }
        if components.count == 1 {
            var dict = object as? [String: Any] ?? [:]
            if dict[first] == nil || dict[first] is NSNull {
                dict[first] = value
            }
            object = dict
            return
        }
        var dict = object as? [String: Any] ?? [:]
        var child: Any = dict[first] ?? [:] as [String: Any]
        setDefaultInObject(&child, components: Array(components.dropFirst()), value: value)
        dict[first] = child
        object = dict
    }

    private func transformEnumInObject(_ object: inout Any, components: [String], mapping: [String: Any], defaultValue: Any?) {
        guard let first = components.first else { return }
        var dict = object as? [String: Any] ?? [:]
        if components.count == 1 {
            if let current = dict[first] {
                dict[first] = mapping[String(describing: current)] ?? defaultValue ?? current
            }
            object = dict
            return
        }
        var child: Any = dict[first] ?? [:] as [String: Any]
        transformEnumInObject(&child, components: Array(components.dropFirst()), mapping: mapping, defaultValue: defaultValue)
        dict[first] = child
        object = dict
    }

    private func executeMicrotestPhase(_ phase: String, steps: [[String: Any]], appId: String) -> [String: Any] {
        var commands: [[String: Any]] = []
        var failures: [[String: Any]] = []
        for (index, step) in steps.enumerated() {
            let execution = executeMicrotestStep(phase: phase, index: index, step: step, appId: appId)
            commands.append(execution)
            if (execution["status"] as? String) == "failed" {
                failures.append(execution)
            }
        }
        return [
            "ok": failures.isEmpty,
            "commands": commands,
            "failures": failures,
        ]
    }

    private func executeMicrotestStep(phase: String, index: Int, step: [String: Any], appId: String) -> [String: Any] {
        let tool = step["tool"] as? String ?? ""
        let normalized = normalizeMicrotestStep(step, appId: appId)
        guard (normalized["mode"] as? String) == "execute" else {
            return [
                "phase": phase,
                "index": index,
                "tool": tool,
                "status": "skipped",
                "reason": normalized["reason"] ?? "UI step validated statically",
            ]
        }
        let args = normalized["args"] as? [String: Any] ?? [:]
        guard let result = executeMicrotestCommand(tool: normalized["tool"] as? String ?? tool, args: args) else {
            return [
                "phase": phase,
                "index": index,
                "tool": tool,
                "status": "failed",
                "error": ["code": "platform.unavailable", "message": "Micro-test command is not executable by the macOS static runner"],
            ]
        }
        if (result["ok"] as? Bool) == false || (result["status"] as? String) == "failed" {
            return [
                "phase": phase,
                "index": index,
                "tool": tool,
                "status": "failed",
                "args": summarizeMicrotestArgs(args),
                "result": summarizeMicrotestCommandResult(result),
            ]
        }
        return [
            "phase": phase,
            "index": index,
            "tool": tool,
            "status": "passed",
            "args": summarizeMicrotestArgs(args),
            "result": summarizeMicrotestCommandResult(result),
        ]
    }

    private func normalizeMicrotestStep(_ step: [String: Any], appId: String) -> [String: Any] {
        let tool = step["tool"] as? String ?? ""
        var args = step["args"] as? [String: Any] ?? [:]
        if let path = args["path"], args["packagePath"] == nil {
            args["packagePath"] = path
        }
        if tool == "runtime.network_mock_set" {
            let match = args["match"] as? [String: Any] ?? [:]
            args["appId"] = args["appId"] ?? appId
            args["urlPattern"] = args["urlPattern"] ?? match["urlPattern"] ?? match["url"]
            args["method"] = args["method"] ?? match["method"] ?? "GET"
        } else if tool == "runtime.dialog_mock_set" {
            args["appId"] = args["appId"] ?? appId
            if args["dialogType"] == nil, let method = args["method"] as? String {
                args["dialogType"] = method.replacingOccurrences(of: "dialog.", with: "")
            }
        } else if tool == "platform.open_webapp" || tool == "platform.create_snapshot" {
            args["appId"] = args["appId"] ?? appId
        } else if [
            "runtime.capabilities",
            "runtime.run_smoke_tests",
            "runtime.resource_usage",
            "runtime.run_accessibility_audit",
            "runtime.accessibility_snapshot",
            "runtime.assert_accessibility",
            "runtime.assert_no_console_errors",
            "runtime.screenshot",
        ].contains(tool) {
            args["appId"] = args["appId"] ?? appId
        }
        if [
            "platform.validate_package",
            "platform.sign_webapp_package",
            "platform.install_webapp_package",
            "platform.open_webapp",
            "platform.create_snapshot",
            "runtime.capabilities",
            "runtime.run_smoke_tests",
            "runtime.resource_usage",
            "runtime.run_accessibility_audit",
            "runtime.accessibility_snapshot",
            "runtime.assert_accessibility",
            "runtime.assert_no_console_errors",
            "runtime.screenshot",
            "runtime.network_mock_set",
            "runtime.dialog_mock_set",
        ].contains(tool) {
            return ["mode": "execute", "tool": tool, "args": args]
        }
        if tool == "runtime.wait_for" || tool == "platform.reset_webapp" {
            return ["mode": "noop", "reason": "not needed for static validation"]
        }
        return ["mode": "noop", "reason": "UI step validated statically"]
    }

    private func executeMicrotestCommand(tool: String, args: [String: Any]) -> [String: Any]? {
        switch tool {
        case "platform.validate_package":
            return validatePackageResult(args: args)
        case "platform.sign_webapp_package":
            return signPackageResult(args: args)
        case "platform.install_webapp_package":
            return installPackageResult(args: args)
        case "platform.open_webapp":
            guard let appId = args["appId"] as? String, activeAppRecord(appId: appId)?.installId != nil else {
                return ["ok": false, "status": "failed", "error": ["code": "app_not_installed"]]
            }
            return ["ok": true, "sessionId": runtimeSessionId(appId: appId), "appId": appId]
        case "platform.create_snapshot":
            guard let appId = args["appId"] as? String else { return ["ok": false, "status": "failed"] }
            return createSnapshot(appId: appId, type: args["type"] as? String ?? "manual", sessionId: args["sessionId"] as? String)
        case "runtime.capabilities":
            return runtimeCapabilities(appId: args["appId"] as? String)
        case "runtime.run_smoke_tests":
            guard let appId = args["appId"] as? String else { return ["ok": false, "status": "failed"] }
            return runSmokeTests(appId: appId)
        case "runtime.resource_usage":
            return resourceUsage(appId: args["appId"] as? String)
        case "runtime.run_accessibility_audit":
            return accessibilityAudit(appId: args["appId"] as? String)
        case "runtime.accessibility_snapshot":
            return accessibilitySnapshot(appId: args["appId"] as? String)
        case "runtime.assert_accessibility":
            let report = accessibilityAudit(appId: args["appId"] as? String)
            let failed = (report["checks"] as? [[String: Any]] ?? []).contains { ($0["status"] as? String) == "fail" }
            return ["ok": !failed, "report": report]
        case "runtime.assert_no_console_errors":
            let errors = consoleLogRows(appId: args["appId"] as? String).filter { row in
                let params = jsonDictionary(row["params_json"] as? String ?? "") ?? [:]
                return (params["level"] as? String) == "error"
            }
            return ["ok": errors.isEmpty, "errors": errors.count]
        case "runtime.screenshot":
            guard let appId = args["appId"] as? String else { return ["ok": false, "status": "failed"] }
            return runtimeScreenshot(appId: appId, label: args["label"] as? String)
        case "runtime.network_mock_set":
            let match = args["match"] as? [String: Any] ?? [:]
            guard let urlPattern = args["urlPattern"] as? String ?? match["urlPattern"] as? String ?? match["url"] as? String else {
                return ["ok": false, "status": "failed"]
            }
            return addNetworkMock(sessionId: args["sessionId"] as? String, appId: args["appId"] as? String, method: (args["method"] as? String ?? "GET").uppercased(), urlPattern: urlPattern, response: args["response"] ?? NSNull())
        case "runtime.dialog_mock_set":
            guard let dialogType = normalizedDialogType(args) else {
                return ["ok": false, "status": "failed"]
            }
            let response = args["response"] ?? ["files": args["files"] ?? [], "selectedPath": args["selectedPath"] ?? NSNull(), "cancelled": args["cancelled"] ?? false]
            return addDialogMock(sessionId: args["sessionId"] as? String, appId: args["appId"] as? String, dialogType: dialogType, response: response)
        default:
            return nil
        }
    }

    private func evaluateMicroTest(appId: String, microtest: [String: Any], commandResults: [[String: Any]]) -> [String: Any] {
        let html = htmlForBundledApp(appId)
        let appJs = bundledAppText(appId: appId, path: "app.js") ?? ""
        var failures: [[String: Any]] = []
        var dynamicText = Set(dynamicTextFromCommands(commandResults))
        let setup = microtest["setup"] as? [[String: Any]] ?? []
        let steps = microtest["steps"] as? [[String: Any]] ?? []
        let teardown = microtest["teardown"] as? [[String: Any]] ?? []
        for step in setup + steps + teardown {
            let tool = step["tool"] as? String ?? ""
            let args = step["args"] as? [String: Any] ?? [:]
            if ["runtime.click", "runtime.type", "runtime.set_value", "runtime.assert_visible"].contains(tool),
               let testId = args["testId"] as? String,
               queryMatches(html: html, args: ["testId": testId]).isEmpty
            {
                failures.append(["tool": tool, "code": "selector.not_found", "testId": testId])
            }
            if tool == "runtime.type", let text = args["text"] {
                dynamicText.insert(String(describing: text))
            }
            if tool == "runtime.set_value", let value = args["value"] {
                dynamicText.insert(String(describing: value))
            }
            if ["runtime.assert_text", "runtime.assert_visible"].contains(tool), let text = args["text"] as? String {
                if !textCanAppear(html: html, dynamicText: dynamicText, text: text) {
                    failures.append(["tool": tool, "code": "text.not_found", "text": text])
                }
            }
            if tool == "runtime.assert_bridge_call", let method = args["method"] as? String, !bridgeMethodReferenced(appJs, method) {
                failures.append(["tool": tool, "code": "bridge.call_missing", "method": method])
            }
            if tool == "runtime.replay_events", !bridgeMethodReferenced(appJs, "core.step") {
                failures.append(["tool": tool, "code": "core.action_missing", "method": "core.step"])
            }
        }
        return [
            "ok": failures.isEmpty,
            "id": microtest["id"] ?? NSNull(),
            "totalSteps": setup.count + steps.count + teardown.count,
            "failures": failures,
        ]
    }

    private func runSmokeTests(appId: String) -> [String: Any]? {
        guard let smokeText = bundledAppText(appId: appId, path: "smoke-tests.json"),
              let tests = bundledSmokeTests(text: smokeText)
        else {
            return nil
        }
        let result = evaluateSmokeTests(appId: appId, tests: tests)
        return recordTestRun(
            microTestId: "smoke:\(appId)",
            name: "\(appId) bundled smoke tests",
            appId: appId,
            spec: tests,
            status: (result["ok"] as? Bool) == true ? "passed" : "failed",
            result: result,
            diagnostics: ["runner": "native-macos-static"]
        )
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

    private func recordBackupExport(_ document: [String: Any], type: String, contentHash: String, createdAt: String) {
        guard let db = database.handle else { return }
        var statement: OpaquePointer?
        let sql = """
        INSERT OR REPLACE INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at)
        VALUES (?, ?, 'macos', '0.4.0', ?, ?, ?)
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, document["exportId"] as? String ?? "")
        bind(statement, 2, type)
        bind(statement, 3, jsonBody(document))
        bind(statement, 4, contentHash)
        bind(statement, 5, createdAt)
        sqlite3_step(statement)
    }

    private func activeAppRecord(appId: String) -> (installId: String?, version: String?, dataVersion: Int)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT active_install_id, active_version, data_version FROM apps WHERE id = ? AND status = 'enabled'"
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

    private func appRecord(appId: String) -> (status: String, activeInstallId: String?, activeVersion: String?, dataVersion: Int)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT status, active_install_id, active_version, data_version FROM apps WHERE id = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            status: columnText(statement, 0),
            activeInstallId: columnNullableText(statement, 1),
            activeVersion: columnNullableText(statement, 2),
            dataVersion: Int(sqlite3_column_int64(statement, 3))
        )
    }

    private func versionRecord(appId: String, installId: String) -> (installId: String, version: String, dataVersion: Int, status: String)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = "SELECT install_id, version, data_version, status FROM app_versions WHERE app_id = ? AND install_id = ?"
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            installId: columnText(statement, 0),
            version: columnText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2)),
            status: columnText(statement, 3)
        )
    }

    private func previousRestorableVersion(appId: String, excluding installId: String) -> (installId: String, version: String, dataVersion: Int)? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = """
        SELECT install_id, version, data_version
        FROM app_versions
        WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled')
        ORDER BY created_at DESC
        LIMIT 1
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            installId: columnText(statement, 0),
            version: columnText(statement, 1),
            dataVersion: Int(sqlite3_column_int64(statement, 2))
        )
    }

    private func quarantineWebapp(appId: String, installId: String?, reason: String, restorePrevious: Bool) -> [String: Any]? {
        guard let app = appRecord(appId: appId), app.status != "uninstalled" else {
            return nil
        }
        guard let targetInstallId = installId ?? app.activeInstallId,
              let target = versionRecord(appId: appId, installId: targetInstallId),
              target.status != "uninstalled"
        else {
            return nil
        }
        let restoreTarget = restorePrevious && app.activeInstallId == targetInstallId
            ? previousRestorableVersion(appId: appId, excluding: targetInstallId)
            : nil
        let now = Self.now()

        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true
        ok = ok && executePrepared(
            "UPDATE app_versions SET status = 'quarantined' WHERE app_id = ? AND install_id = ?",
            [appId, targetInstallId]
        )
        if let restoreTarget {
            ok = ok && executePrepared(
                "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?",
                [now, restoreTarget.installId]
            )
            ok = ok && executePrepared(
                "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
                [restoreTarget.installId, restoreTarget.version, restoreTarget.dataVersion, now, appId]
            )
            ok = ok && executePrepared(
                """
                INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json)
                VALUES (?, ?, ?, 'rollback', ?, 'codex', ?, ?)
                """,
                [
                    "event_\(UUID().uuidString.lowercased())",
                    appId,
                    restoreTarget.installId,
                    targetInstallId,
                    now,
                    jsonBody(["reason": "automatic rollback after quarantine", "quarantinedInstallId": targetInstallId] as [String: Any]),
                ]
            )
        } else if app.activeInstallId == targetInstallId {
            ok = ok && executePrepared(
                "UPDATE apps SET status = 'quarantined', updated_at = ? WHERE id = ?",
                [now, appId]
            )
        }
        ok = ok && executePrepared(
            """
            INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json)
            VALUES (?, ?, ?, 'quarantine', ?, 'codex', ?, ?)
            """,
            [
                "event_\(UUID().uuidString.lowercased())",
                appId,
                targetInstallId,
                restoreTarget?.installId,
                now,
                jsonBody(["reason": reason, "restoredInstallId": restoreTarget?.installId ?? NSNull()] as [String: Any]),
            ]
        )

        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            return nil
        }
        return [
            "appId": appId,
            "installId": targetInstallId,
            "status": "quarantined",
            "reason": reason,
            "restoredInstallId": restoreTarget?.installId ?? NSNull(),
        ]
    }

    private func uninstallWebapp(appId: String) -> [String: Any]? {
        guard let app = appRecord(appId: appId), app.status != "uninstalled" else {
            return nil
        }
        let clearedStorageKeys = scalarInt("SELECT COUNT(*) FROM app_storage WHERE app_id = ?", values: [appId])
        let snapshot = createSnapshot(appId: appId, type: "manual", sessionId: activeAppId == appId ? activeRuntimeSessionId : nil)
        let now = Self.now()

        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true
        ok = ok && executePrepared("DELETE FROM app_storage WHERE app_id = ?", [appId])
        ok = ok && executePrepared("UPDATE app_versions SET status = 'uninstalled' WHERE app_id = ?", [appId])
        ok = ok && executePrepared(
            "UPDATE apps SET status = 'uninstalled', active_install_id = NULL, active_version = NULL, updated_at = ? WHERE id = ?",
            [now, appId]
        )
        if let activeInstallId = app.activeInstallId {
            ok = ok && executePrepared(
                """
                INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json)
                VALUES (?, ?, ?, 'uninstall', ?, 'codex', ?, ?)
                """,
                [
                    "event_\(UUID().uuidString.lowercased())",
                    appId,
                    activeInstallId,
                    activeInstallId,
                    now,
                    jsonBody(["snapshotId": snapshot?["snapshotId"] ?? NSNull(), "clearedStorageKeys": clearedStorageKeys] as [String: Any]),
                ]
            )
        }
        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            return nil
        }
        if activeAppId == appId {
            activeAppId = nil
            activeRuntimeSessionId = nil
        }
        return [
            "ok": true,
            "appId": appId,
            "status": "uninstalled",
            "snapshotId": snapshot?["snapshotId"] ?? NSNull(),
            "clearedStorageKeys": clearedStorageKeys,
        ]
    }

    private func approveWebappUpdate(appId: String, installId: String) -> [String: Any]? {
        guard let target = versionRecord(appId: appId, installId: installId),
              target.status != "quarantined",
              target.status != "uninstalled",
              let report = installReportRecord(appId: appId, installId: installId),
              report.status == "requires-approval"
        else {
            return nil
        }
        let previousInstallId = activeAppRecord(appId: appId)?.installId
        let migrationRuns = applyPendingInstallMigrations(appId: appId, target: target)
        if migrationRuns == nil {
            return nil
        }
        let approvedAt = Self.now()
        var permissionsReport = report.permissions
        permissionsReport["requiresUserApproval"] = true
        permissionsReport["approvalGranted"] = true
        permissionsReport["approvedAt"] = approvedAt
        permissionsReport["approved"] = permissionsForInstall(installId: installId)

        guard executeSQL("BEGIN IMMEDIATE") else { return nil }
        var ok = true
        if let previousInstallId, previousInstallId != installId {
            ok = ok && executePrepared("UPDATE app_versions SET status = 'installed' WHERE install_id = ?", [previousInstallId])
        }
        ok = ok && executePrepared(
            "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?",
            [approvedAt, installId]
        )
        ok = ok && executePrepared(
            "UPDATE app_permissions SET approved = 1, approved_at = ?, reason = 'approved update' WHERE install_id = ?",
            [approvedAt, installId]
        )
        ok = ok && executePrepared(
            "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
            [installId, target.version, target.dataVersion, approvedAt, appId]
        )
        ok = ok && executePrepared(
            "UPDATE app_install_reports SET status = 'accepted', permissions_json = ? WHERE report_id = ?",
            [jsonBody(permissionsReport), report.reportId]
        )
        ok = ok && executePrepared(
            """
            INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json)
            VALUES (?, ?, ?, 'activate', ?, 'codex', ?, ?, ?)
            """,
            [
                "event_\(UUID().uuidString.lowercased())",
                appId,
                installId,
                previousInstallId,
                report.reportId,
                approvedAt,
                jsonBody(["approved": true, "previousInstallId": previousInstallId ?? NSNull(), "migrationRuns": migrationRuns ?? []] as [String: Any]),
            ]
        )
        guard ok, executeSQL("COMMIT") else {
            _ = executeSQL("ROLLBACK")
            return nil
        }
        return [
            "appId": appId,
            "installId": installId,
            "status": "enabled",
            "previousInstallId": previousInstallId ?? NSNull(),
            "migrationRuns": migrationRuns ?? [],
        ]
    }

    private func applyPendingInstallMigrations(
        appId: String,
        target: (installId: String, version: String, dataVersion: Int, status: String)
    ) -> [[String: Any]]? {
        guard let active = activeAppRecord(appId: appId) else {
            return []
        }
        guard target.dataVersion > active.dataVersion else {
            return []
        }
        var runs: [[String: Any]] = []
        for fromVersion in active.dataVersion..<target.dataVersion {
            let path = "migrations/\(fromVersion)_to_\(fromVersion + 1).json"
            guard let migration = packageFileJSON(installId: target.installId, path: path) else {
                return nil
            }
            do {
                runs.append(try runMigration(migration: migration, mode: "apply"))
            } catch {
                return nil
            }
        }
        return runs
    }

    private func packageFileJSON(installId: String, path: String) -> [String: Any]? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT content_text FROM app_files WHERE install_id = ? AND path = ?", -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, installId)
        bind(statement, 2, path)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return jsonDictionary(columnText(statement, 0))
    }

    private func installReportRecord(appId: String, installId: String) -> (reportId: String, status: String, permissions: [String: Any])? {
        guard let db = database.handle else { return nil }
        var statement: OpaquePointer?
        let sql = """
        SELECT report_id, status, permissions_json
        FROM app_install_reports
        WHERE app_id = ? AND install_id = ?
        ORDER BY created_at DESC
        LIMIT 1
        """
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return nil
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, installId)
        guard sqlite3_step(statement) == SQLITE_ROW else {
            return nil
        }
        return (
            reportId: columnText(statement, 0),
            status: columnText(statement, 1),
            permissions: jsonDictionary(columnText(statement, 2)) ?? [:]
        )
    }

    private func permissionsForInstall(installId: String) -> [String] {
        guard let db = database.handle else { return [] }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT permission FROM app_permissions WHERE install_id = ? ORDER BY permission", -1, &statement, nil) == SQLITE_OK else {
            return []
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, installId)
        var permissions: [String] = []
        while sqlite3_step(statement) == SQLITE_ROW {
            permissions.append(columnText(statement, 0))
        }
        return permissions
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

    private func executePrepared(_ sql: String, _ values: [Any?]) -> Bool {
        guard let db = database.handle else { return false }
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return false
        }
        defer { sqlite3_finalize(statement) }
        for (index, value) in values.enumerated() {
            bindAny(statement, Int32(index + 1), value)
        }
        return sqlite3_step(statement) == SQLITE_DONE
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

    private func mockedNetworkRequestTimeoutMs(_ params: [String: Any]) -> (value: Int?, invalidValue: Any?) {
        guard let rawValue = params["timeoutMs"] else {
            return (nil, nil)
        }
        guard let timeoutMs = positiveInteger(rawValue) else {
            return (nil, rawValue)
        }
        return (timeoutMs, nil)
    }

    private func mockedNetworkDelayMs(_ value: Any) -> Int? {
        guard let object = value as? [String: Any],
              let rawDelay = object["delayMs"]
        else {
            return nil
        }
        return positiveInteger(rawDelay)
    }

    private func effectiveMockedNetworkTimeoutMs(rule: NetworkPolicyRule, requestedTimeoutMs: Int?) -> Int {
        requestedTimeoutMs.map { min(rule.timeoutMs, $0) } ?? rule.timeoutMs
    }

    private func positiveInteger(_ value: Any) -> Int? {
        if value is Bool {
            return nil
        }
        if let intValue = value as? Int {
            return intValue > 0 ? intValue : nil
        }
        if let doubleValue = value as? Double {
            return doubleValue.isFinite && doubleValue > 0 && doubleValue <= Double(Int.max) && doubleValue.rounded(.towardZero) == doubleValue
                ? Int(doubleValue)
                : nil
        }
        if let number = value as? NSNumber {
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return nil
            }
            let doubleValue = number.doubleValue
            return doubleValue.isFinite && doubleValue > 0 && doubleValue <= Double(Int.max) && doubleValue.rounded(.towardZero) == doubleValue
                ? Int(doubleValue)
                : nil
        }
        return nil
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

    private func firstValue(_ object: [String: Any], keys: [String]) -> Any? {
        for key in keys {
            if let value = object[key], !(value is NSNull) {
                return value
            }
        }
        return nil
    }

    private func textValue(_ object: [String: Any], keys: [String]) -> String? {
        guard let value = firstValue(object, keys: keys) else {
            return nil
        }
        if let value = value as? String {
            return value
        }
        if let value = value as? NSNumber {
            return value.stringValue
        }
        return nil
    }

    private func jsonTextValue(
        _ object: [String: Any],
        stringKeys: [String],
        objectKeys: [String],
        fallback: String?
    ) -> String? {
        if let text = textValue(object, keys: stringKeys) {
            return text
        }
        if let value = firstValue(object, keys: objectKeys) {
            return jsonString(value)
        }
        return fallback
    }

    private func sortedStringArray(_ value: Any?) -> [String] {
        (value as? [String] ?? []).sorted()
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
        bundledAppText(appId: appId, path: "index.html") ?? ""
    }

    private func bundledAppText(appId: String, path: String) -> String? {
        let fileURL = RuntimeResourceLocator.repoRootURL()
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent(path)
        return try? String(contentsOf: fileURL, encoding: .utf8)
    }

    private func bundledSmokeTests(text: String) -> [[String: Any]]? {
        guard let data = text.data(using: .utf8),
              let tests = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else {
            return nil
        }
        return tests
    }

    private func evaluateSmokeTests(appId: String, tests: [[String: Any]]) -> [String: Any] {
        let html = htmlForBundledApp(appId)
        let appJs = bundledAppText(appId: appId, path: "app.js") ?? ""
        var failures: [[String: Any]] = []
        var dynamicText = Set<String>()

        for test in tests {
            let testName = test["name"] as? String ?? "unnamed"
            for step in test["steps"] as? [[String: Any]] ?? [] {
                if let selector = step["selector"] as? String,
                   queryMatches(html: html, args: ["selector": selector]).isEmpty
                {
                    failures.append(["test": testName, "code": "selector.not_found", "selector": selector])
                }
                if let type = step["type"] as? String,
                   (type == "fill" || type == "select"),
                   let value = step["value"] as? String
                {
                    dynamicText.insert(value)
                }
            }

            let expected = test["expected"] as? [String: Any] ?? [:]
            for method in expected["bridgeCallsInclude"] as? [String] ?? [] {
                if !bridgeMethodReferenced(appJs, method) {
                    failures.append(["test": testName, "code": "bridge.call_missing", "method": method])
                }
            }
            if let text = expected["textIncludes"] as? String,
               !textCanAppear(html: html, dynamicText: dynamicText, text: text)
            {
                failures.append(["test": testName, "code": "text.not_found", "text": text])
            }
        }

        return [
            "ok": failures.isEmpty,
            "appId": appId,
            "total": tests.count,
            "assertions": tests.reduce(0) { count, test in
                let steps = (test["steps"] as? [[String: Any]])?.count ?? 0
                let expected = test["expected"] as? [String: Any] ?? [:]
                return count + steps + expected.keys.count
            },
            "failures": failures,
            "runner": "static",
        ]
    }

    private func bridgeMethodReferenced(_ appJs: String, _ method: String) -> Bool {
        appJs.contains(method)
    }

    private func textCanAppear(html: String, dynamicText: Set<String>, text: String) -> Bool {
        htmlText(html).contains(text) || dynamicText.contains(text)
    }

    private func smokeResult(package: PackageRead) -> [String: Any] {
        guard let appId = package.manifest["id"] as? String else {
            return ["ok": false, "status": "failed", "failures": [["code": "invalid_manifest_id"]], "spec": []]
        }
        guard let smokeText = package.files.first(where: { $0.path == "smoke-tests.json" })?.content else {
            return ["ok": true, "status": "not-run", "appId": appId, "total": 0, "failures": [], "spec": []]
        }
        guard let tests = bundledSmokeTests(text: smokeText) else {
            return ["ok": false, "status": "failed", "appId": appId, "total": 0, "failures": [["code": "package.invalid", "path": "smoke-tests.json"]], "spec": []]
        }
        var result = evaluateSmokeTests(appId: appId, tests: tests)
        result["status"] = (result["ok"] as? Bool) == true ? "passed" : "failed"
        result["spec"] = tests
        return result
    }

    private func accessibilityAuditForHTML(appId: String, html: String) -> [String: Any] {
        let title = firstMatch(in: html, pattern: #"<title[^>]*>([\s\S]*?)</title>"#)
        let landmarks: [[String: Any]] = html.range(of: #"<main\b"#, options: [.regularExpression, .caseInsensitive]) == nil ? [] : [
            ["role": "main", "selector": "main"],
        ]
        let headings = headingRecords(html)
        let controls = controlRecords(html)
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
        return [
            "appId": appId,
            "checkedAt": Self.now(),
            "status": checks.contains { $0["status"] as? String == "fail" } ? "fail" : "pass",
            "checks": checks,
        ]
    }

    private func runtimeCompatibilityResult(_ appRuntimeVersion: String?) -> [String: Any] {
        [
            "ok": appRuntimeVersion == nil || appRuntimeVersion == "0.1.0",
            "runtimeVersion": "0.1.0",
            "appRuntimeVersion": appRuntimeVersion ?? NSNull(),
            "allowRuntimeMismatch": false,
        ]
    }

    private func packageHashes(_ package: PackageRead) -> [String: String] {
        packageHashes(
            manifest: package.manifest,
            files: package.files.map { (path: $0.path, content: $0.content) },
            permissions: package.manifest["permissions"] as? [String] ?? []
        )
    }

    private func packageHashes(
        manifest: [String: Any],
        files: [(path: String, content: String)],
        permissions: [String]
    ) -> [String: String] {
        let manifestHash = "sha256:\(sha256Hex(jsonString(manifest)))"
        let content = files
            .sorted { $0.path < $1.path }
            .map { "\($0.path)\nsha256:\(sha256Hex($0.content))\n" }
            .joined()
        let sortedPermissions = permissions.sorted()
        let policy: [String: Any] = [
            "capabilities": manifest["capabilities"] ?? [:],
            "networkPolicy": manifest["networkPolicy"] ?? [:],
            "resourceBudget": manifest["resourceBudget"] ?? [:],
        ]
        return [
            "manifestHash": manifestHash,
            "contentHash": "sha256:\(sha256Hex(content))",
            "permissionsHash": "sha256:\(sha256Hex(jsonString(sortedPermissions)))",
            "policyHash": "sha256:\(sha256Hex(jsonString(policy)))",
        ]
    }

    private static func loadOrCreateSigningKey(account: String) throws -> Curve25519.Signing.PrivateKey {
        if let key = try loadSigningKey(account: account) {
            return key
        }
        let key = Curve25519.Signing.PrivateKey()
        try storeSigningKey(key, account: account)
        return key
    }

    private static func loadSigningKey(account: String) throws -> Curve25519.Signing.PrivateKey? {
        var query = signingKeyQuery(account: account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw ControlError.signingKeyUnavailable(status)
        }
        guard let data = item as? Data else {
            throw ControlError.signingKeyUnavailable(errSecDecode)
        }
        do {
            return try Curve25519.Signing.PrivateKey(rawRepresentation: data)
        } catch {
            _ = SecItemDelete(signingKeyQuery(account: account) as CFDictionary)
            return nil
        }
    }

    private static func storeSigningKey(_ key: Curve25519.Signing.PrivateKey, account: String) throws {
        var attributes = signingKeyQuery(account: account)
        attributes[kSecValueData as String] = key.rawRepresentation
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlocked

        let status = SecItemAdd(attributes as CFDictionary, nil)
        if status == errSecSuccess {
            return
        }
        if status == errSecDuplicateItem {
            let updateStatus = SecItemUpdate(
                signingKeyQuery(account: account) as CFDictionary,
                [kSecValueData as String: key.rawRepresentation] as CFDictionary
            )
            guard updateStatus == errSecSuccess else {
                throw ControlError.signingKeyUnavailable(updateStatus)
            }
            return
        }
        throw ControlError.signingKeyUnavailable(status)
    }

    private static func signingKeyQuery(account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: signingKeyService,
            kSecAttrAccount as String: account,
        ]
    }

    private func signingKeyId() -> String {
        "platform-host:macos:\(sha256Hex(signingKey.publicKey.rawRepresentation).prefix(16))"
    }

    private func signingPublicKeyDescriptor() -> [String: Any] {
        [
            "algorithm": "ed25519",
            "keyId": signingKeyId(),
            "format": "raw",
            "publicKey": signingKey.publicKey.rawRepresentation.base64EncodedString(),
            "storage": "keychain",
        ]
    }

    private func signPayload(_ payload: String) -> String {
        (try? signingKey.signature(for: Data(payload.utf8)).base64EncodedString()) ?? ""
    }

    private func signaturePayload(
        appId: String,
        appVersion: String,
        dataVersion: Int,
        runtimeVersion: String,
        trustLevel: String,
        keyId: String,
        manifestHash: String,
        contentHash: String,
        permissionsHash: String,
        policyHash: String,
        signedAt: String
    ) -> String {
        [
            "native-ai-webapp/sig/v1",
            appId,
            appVersion,
            String(dataVersion),
            runtimeVersion,
            trustLevel,
            keyId,
            manifestHash,
            contentHash,
            permissionsHash,
            policyHash,
            signedAt,
        ].joined(separator: "\n")
    }

    private func bridgeMethods(in appJs: String) -> Set<String> {
        let known = [
            "core.step",
            "storage.get",
            "storage.list",
            "storage.set",
            "storage.remove",
            "dialog.openFile",
            "dialog.saveFile",
            "notification.toast",
            "network.request",
            "app.log",
            "runtime.capabilities",
        ]
        return Set(known.filter { bridgeMethodReferenced(appJs, $0) })
    }

    private func packageIssue(_ code: String, _ message: String, details: [String: Any]) -> [String: Any] {
        [
            "code": code,
            "message": message,
            "details": details,
        ]
    }

    private func mimeType(forPackagePath path: String) -> String {
        if path.hasSuffix(".html") { return "text/html" }
        if path.hasSuffix(".css") { return "text/css" }
        if path.hasSuffix(".js") { return "text/javascript" }
        if path.hasSuffix(".json") { return "application/json" }
        return "text/plain"
    }

    private func repoRelativeURL(_ path: String) -> URL? {
        let root = RuntimeResourceLocator.repoRootURL().standardizedFileURL
        let url = (path.hasPrefix("/") ? URL(fileURLWithPath: path) : root.appendingPathComponent(path)).standardizedFileURL
        guard url.path == root.path || url.path.hasPrefix(root.path + "/") else {
            return nil
        }
        return url
    }

    private func summarizeMicrotestCommandResult(_ result: [String: Any]) -> [String: Any] {
        var summary: [String: Any] = ["ok": result["ok"] ?? true]
        for key in ["appId", "installId", "sessionId", "keyId", "status", "snapshotId", "reportId"] {
            if let value = result[key] {
                summary[key] = value
            }
        }
        return summary
    }

    private func summarizeMicrotestArgs(_ args: [String: Any]) -> [String: Any] {
        var summary = args
        if let path = summary["packagePath"] as? String, let url = repoRelativeURL(path) {
            summary["packagePath"] = relativeRepoPath(url)
        }
        if let path = summary["path"] as? String, let url = repoRelativeURL(path) {
            summary["path"] = relativeRepoPath(url)
        }
        return summary
    }

    private func relativeRepoPath(_ url: URL) -> String {
        let root = RuntimeResourceLocator.repoRootURL().standardizedFileURL.path
        let path = url.standardizedFileURL.path
        if path.hasPrefix(root + "/") {
            return String(path.dropFirst(root.count + 1))
        }
        return path
    }

    private func dynamicTextFromCommands(_ commands: [[String: Any]]) -> [String] {
        var values: [String] = []
        for command in commands {
            collectText(command["args"], values: &values)
            collectText(command["result"], values: &values)
        }
        return values
    }

    private func collectText(_ value: Any?, values: inout [String]) {
        guard let value, !(value is NSNull) else { return }
        if let string = value as? String {
            values.append(string)
            return
        }
        if let number = value as? NSNumber {
            values.append(number.stringValue)
            return
        }
        if let array = value as? [Any] {
            for item in array {
                collectText(item, values: &values)
            }
            return
        }
        if let dict = value as? [String: Any] {
            for item in dict.values {
                collectText(item, values: &values)
            }
        }
    }

    private func expandPlatformSmokeStep(_ step: [String: Any], values: [String: String]) -> [String: Any] {
        var expanded = step
        expanded["args"] = expandPlaceholders(step["args"] as? [String: Any] ?? [:], values: values)
        return expanded
    }

    private func expandPlaceholders(_ value: Any, values: [String: String]) -> Any {
        if let string = value as? String {
            var output = string
            for (name, replacement) in values {
                output = output.replacingOccurrences(of: "${\(name)}", with: replacement)
            }
            return output
        }
        if let array = value as? [Any] {
            return array.map { expandPlaceholders($0, values: values) }
        }
        if let dict = value as? [String: Any] {
            return dict.mapValues { expandPlaceholders($0, values: values) }
        }
        return value
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

    private func bindAny(_ statement: OpaquePointer?, _ index: Int32, _ value: Any?) {
        guard let value, !(value is NSNull) else {
            sqlite3_bind_null(statement, index)
            return
        }
        if let value = value as? String {
            bind(statement, index, value)
        } else if let value = value as? Int {
            sqlite3_bind_int64(statement, index, Int64(value))
        } else if let value = value as? Int64 {
            sqlite3_bind_int64(statement, index, value)
        } else if let value = value as? Bool {
            sqlite3_bind_int64(statement, index, value ? 1 : 0)
        } else if let value = value as? NSNumber {
            sqlite3_bind_int64(statement, index, value.int64Value)
        } else {
            bind(statement, index, jsonString(value))
        }
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
    sha256Hex(Data(text.utf8))
}

private func sha256Hex(_ data: Data) -> String {
    let digest = SHA256.hash(data: data)
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
