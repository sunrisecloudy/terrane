import Foundation
import SQLite3
import WebKit

@MainActor
final class WebBridge: NSObject, WKScriptMessageHandlerWithReply {
    private let storage = PlatformStorage()
    private let dialogs = PlatformDialogs()
    private let notifications = PlatformNotifications()
    private let network = PlatformNetwork()
    private let core = ForgeCoreBridge()
    private var nativeDevMode: Bool {
#if DEBUG
        true
#else
        false
#endif
    }
    private static let runtimeEnvelopeFields: Set<String> = ["appId", "mountToken", "request"]
    private static let bridgeRequestFields: Set<String> = ["id", "method", "params", "timestamp"]

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any] else {
            replyHandler(BridgeResponse.failure(id: nil, code: "invalid_request", message: "Bridge message body must be an object").asDictionary(), nil)
            return
        }

        let envelope = BridgeEnvelope(body: body)
        let sandboxContext = AppSandboxContext(message: message, envelope: envelope, core: core)
        if let gateFailure = envelopeGateFailure(body: body, message: message, context: sandboxContext) {
            replyHandler(gateFailure.asDictionary(), nil)
            return
        }

        let request = BridgeRequest(body: envelope.requestBody, context: sandboxContext)
        let startedAt = Date()
        if request.method == "core.step" {
            core.stepAsync(request) { [weak self] result in
                Task { @MainActor in
                    guard let self else {
                        replyHandler(result.asDictionary(), nil)
                        return
                    }
                    self.recordBridgeCall(request: request, response: result, startedAt: startedAt)
                    self.recordCoreStep(request: request, response: result)
                    replyHandler(result.asDictionary(), nil)
                }
            }
            return
        }

        let result = dispatch(request)
        recordBridgeCall(request: request, response: result, startedAt: startedAt)
        recordCoreStep(request: request, response: result)
        replyHandler(result.asDictionary(), nil)
    }

    private func dispatch(_ request: BridgeRequest) -> BridgeResponse {
        switch request.method {
        case "storage.get":
            return storage.get(request)
        case "storage.set":
            return storage.set(request)
        case "storage.remove":
            return storage.remove(request)
        case "storage.list":
            return storage.list(request)
        case "dialog.openFile":
            return dialogs.openFile(request)
        case "dialog.saveFile":
            return dialogs.saveFile(request)
        case "notification.toast":
            return notifications.toast(request)
        case "network.request":
            return network.request(request)
        case "core.step":
            return core.step(request)
        case "runtime.capabilities":
            let catalog = ForgeDataCatalog.shared
            var limits: [String: Any] = [
                "maxPackageBytes": catalog.runtimeConfig.maxPackageBytes,
                "maxFileBytes": catalog.runtimeConfig.maxFileBytes
            ]
            for (key, value) in request.context.resourceBudget {
                limits[key] = value
            }
            return .success(id: request.id, result: [
                "platform": catalog.runtimeConfig.platform,
                "target": catalog.runtimeConfig.target,
                "appId": request.context.appId,
                "runtimeVersion": catalog.runtimeVersion,
                "devMode": nativeDevMode,
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
                    "notebook.crdt": core.smokeSyncExport(),
                    "runtime.capabilities": true,
                    "app.log": true
                ],
                "limits": limits
            ])
        case "app.log":
            return appLog(request)
        default:
            return .failure(id: request.id, code: "unknown_method", message: "Unknown bridge method: \(request.method)")
        }
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
        NSLog("Generated app log [\(level)]: \(message)")
        return .success(id: request.id, result: ["ok": true])
    }

    private func envelopeGateFailure(
        body: [String: Any],
        message: WKScriptMessage,
        context: AppSandboxContext
    ) -> BridgeResponse? {
        if let decision = coreEnvelopeDecision(body: body, message: message, context: context) {
            if decision["allowed"] as? Bool == true {
                return nil
            }
            return BridgeResponse.failure(
                id: decision["request_id"] as? String,
                code: decision["error_code"] as? String ?? "invalid_request",
                message: decision["message"] as? String ?? "Bridge request denied",
                details: decision["details"] as? [String: Any] ?? [:]
            )
        }
        return legacyEnvelopeGateFailure(body: body, message: message, context: context)
    }

    private func coreEnvelopeDecision(
        body: [String: Any],
        message: WKScriptMessage,
        context: AppSandboxContext
    ) -> [String: Any]? {
        guard core.isAvailable else { return nil }
        let requestBody = body["request"] as? [String: Any] ?? body
        let storageKey = requestBody["params"] as? [String: Any]
        return core.bridgeCommandDictionary(
            name: "bridge.validate_envelope",
            payload: [
                "input": [
                    "envelope": body,
                    "is_main_frame": message.frameInfo.isMainFrame,
                    "app_id": context.appId,
                    "permissions": Array(context.approvedPermissions).sorted(),
                    "resource_budget": context.resourceBudgetPayload,
                    "storage_prefix": context.storagePrefix,
                    "counts": bridgeCallCounts(appId: context.appId),
                    "storage_key": (storageKey?["key"] as? String ?? storageKey?["prefix"] as? String) as Any,
                ],
            ],
            requestId: requestBody["id"] as? String ?? "macos-envelope-gate"
        )
    }

    private func legacyEnvelopeGateFailure(
        body: [String: Any],
        message: WKScriptMessage,
        context: AppSandboxContext
    ) -> BridgeResponse? {
        let envelope = BridgeEnvelope(body: body)
        if envelope.isRuntimeEnvelope && !message.frameInfo.isMainFrame {
            return .failure(
                id: envelope.requestId,
                code: "bridge.unauthorized_channel",
                message: "Runtime bridge envelope must come from the main runtime frame"
            )
        }
        if envelope.isRuntimeEnvelope && !Self.hasOnlyRuntimeEnvelopeFields(body) {
            return .failure(
                id: envelope.requestId,
                code: "invalid_request",
                message: "Runtime bridge envelope contains unknown top-level fields",
                details: ["fields": Self.extraFields(in: body, allowed: Self.runtimeEnvelopeFields)]
            )
        }
        if envelope.isRuntimeEnvelope && !envelope.hasValidContext {
            return .failure(
                id: envelope.requestId,
                code: "invalid_request",
                message: "Runtime bridge envelope requires appId, mountToken, and request"
            )
        }
        if let validationFailure = Self.bridgeRequestValidationFailure(envelope.requestBody) {
            return validationFailure
        }
        let request = BridgeRequest(body: envelope.requestBody, context: context)
        if request.params["appId"] != nil {
            return .failure(
                id: request.id,
                code: "invalid_request",
                message: "Bridge params must not include appId; app id is channel-derived",
                details: ["field": "appId"]
            )
        }
        if let permission = permissionForBridgeMethod(request.method),
           !request.context.approvedPermissions.contains(permission) {
            return .failure(
                id: request.id,
                code: "permission_denied",
                message: "App \(request.context.appId) cannot call \(request.method)",
                details: ["appId": request.context.appId, "method": request.method, "requiredPermission": permission]
            )
        }
        return bridgeRateBudgetFailure(request)
    }

    private func bridgeCallCounts(appId: String) -> [String: Any] {
        [
            "total_last_60s": bridgeCallCount(appId: appId, seconds: 60),
            "network_last_60s": bridgeCallCount(appId: appId, method: "network.request", seconds: 60),
            "app_log_last_60s": bridgeCallCount(appId: appId, method: "app.log", seconds: 60),
        ]
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
        if request.method == "app.log",
           let limit = request.context.resourceBudget["maxLogLinesPerMinute"] {
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
        return nil
    }

    private static func extraFields(in body: [String: Any], allowed: Set<String>) -> [String] {
        body.keys.filter { !allowed.contains($0) }.sorted()
    }

    private static func hasOnlyRuntimeEnvelopeFields(_ body: [String: Any]) -> Bool {
        extraFields(in: body, allowed: runtimeEnvelopeFields).isEmpty
    }

    private static func hasOnlyBridgeRequestFields(_ body: [String: Any]) -> Bool {
        extraFields(in: body, allowed: bridgeRequestFields).isEmpty
    }

    private static func bridgeRequestValidationFailure(_ body: [String: Any]) -> BridgeResponse? {
        if !hasOnlyBridgeRequestFields(body) {
            return .failure(
                id: nil,
                code: "invalid_request",
                message: "Bridge request contains unknown top-level fields",
                details: ["fields": extraFields(in: body, allowed: bridgeRequestFields)]
            )
        }
        guard let id = body["id"] as? String, !id.isEmpty else {
            return .failure(id: nil, code: "invalid_request", message: "Bridge request id must be a non-empty string")
        }
        if let timestamp = body["timestamp"], !isFiniteJSONNumber(timestamp) {
            return .failure(id: nil, code: "invalid_request", message: "Bridge request timestamp must be a finite number")
        }
        if body["method"] as? String == nil {
            return .failure(id: nil, code: "invalid_request", message: "Bridge request method must be a string")
        }
        if body["params"] as? [String: Any] == nil {
            return .failure(id: nil, code: "invalid_request", message: "Bridge request params must be an object")
        }
        return nil
    }

    private static func isFiniteJSONNumber(_ value: Any) -> Bool {
        if value is Bool {
            return false
        }
        if let number = value as? NSNumber {
            return number.doubleValue.isFinite
        }
        if let double = value as? Double {
            return double.isFinite
        }
        if let float = value as? Float {
            return float.isFinite
        }
        return false
    }

    private func bridgeCallCount(appId: String, seconds: Int) -> Int {
        guard let db = storage.databaseHandle else { return 0 }
        let sql = "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND datetime(created_at) >= datetime('now', ?)"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, "-\(seconds) seconds")
        return sqlite3_step(statement) == SQLITE_ROW ? Int(sqlite3_column_int(statement, 0)) : 0
    }

    private func bridgeCallCount(appId: String, method: String, seconds: Int) -> Int {
        guard let db = storage.databaseHandle else { return 0 }
        let sql = "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND datetime(created_at) >= datetime('now', ?)"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, appId)
        bind(statement, 2, method)
        bind(statement, 3, "-\(seconds) seconds")
        return sqlite3_step(statement) == SQLITE_ROW ? Int(sqlite3_column_int(statement, 0)) : 0
    }

    private func recordBridgeCall(request: BridgeRequest, response: BridgeResponse, startedAt: Date) {
        guard let db = storage.databaseHandle, !request.context.appId.isEmpty else { return }
        ensureRuntimeSession(request)
        let activeInstallId = BridgeBudgetQuarantine.activeInstallId(database: db, appId: request.context.appId)
        let sessionId = runtimeSessionId(request)
        let durationMs = Int64(Date().timeIntervalSince(startedAt) * 1000)
        let record = coreRecordBridgeCall(
            request: request,
            response: response,
            sessionId: sessionId,
            installId: activeInstallId,
            durationMs: durationMs
        )
        let sql = """
        INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
        """
        do {
            var statement: OpaquePointer?
            guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
            defer { sqlite3_finalize(statement) }
            bind(statement, 1, record.bridgeCallId)
            bind(statement, 2, record.sessionId)
            bind(statement, 3, record.appId)
            bindNullable(statement, 4, record.installId)
            bind(statement, 5, record.method)
            bind(statement, 6, record.paramsJSON)
            bindNullable(statement, 7, record.resultJSON)
            bindNullable(statement, 8, record.errorJSON)
            sqlite3_bind_int64(statement, 9, record.durationMs)
            guard sqlite3_step(statement) == SQLITE_DONE else { return }
        }
        BridgeBudgetQuarantine.maybeQuarantineAfterBudgetError(
            database: db,
            appId: request.context.appId,
            installId: activeInstallId,
            error: response.error,
            actor: "macos-runtime"
        )
    }

    private struct BridgeCallInsertRecord {
        let bridgeCallId: String
        let sessionId: String
        let appId: String
        let installId: String?
        let method: String
        let paramsJSON: String
        let resultJSON: String?
        let errorJSON: String?
        let durationMs: Int64
    }

    private func coreRecordBridgeCall(
        request: BridgeRequest,
        response: BridgeResponse,
        sessionId: String,
        installId: String?,
        durationMs: Int64
    ) -> BridgeCallInsertRecord {
        if let payload = core.bridgeCommandDictionary(
            name: "bridge.record_call",
            payload: [
                "record": [
                    "platform_ids": ForgeCoreBridge.bridgePlatformIds(),
                    "session_id": sessionId,
                    "request_id": request.id ?? "unknown",
                    "app_id": request.context.appId,
                    "install_id": installId as Any,
                    "method": request.method,
                    "params": request.params,
                    "ok": response.ok,
                    "result": response.result as Any,
                    "error": response.error as Any,
                    "duration_ms": durationMs,
                ],
            ],
            requestId: request.id ?? "macos-record-call"
        ) {
            return BridgeCallInsertRecord(
                bridgeCallId: payload["bridge_call_id"] as? String ?? legacyBridgeCallId(request),
                sessionId: payload["session_id"] as? String ?? sessionId,
                appId: payload["app_id"] as? String ?? request.context.appId,
                installId: payload["install_id"] as? String ?? installId,
                method: payload["method"] as? String ?? request.method,
                paramsJSON: jsonString(payload["params_json"] ?? request.params),
                resultJSON: jsonOptionalString(payload["result_json"]),
                errorJSON: jsonOptionalString(payload["error_json"]),
                durationMs: (payload["duration_ms"] as? NSNumber)?.int64Value ?? durationMs
            )
        }
        return BridgeCallInsertRecord(
            bridgeCallId: legacyBridgeCallId(request),
            sessionId: sessionId,
            appId: request.context.appId,
            installId: installId,
            method: request.method,
            paramsJSON: jsonString(request.params),
            resultJSON: response.result.map(jsonString),
            errorJSON: response.error.map(jsonString),
            durationMs: durationMs
        )
    }

    private func legacyBridgeCallId(_ request: BridgeRequest) -> String {
        "bridge_macos_\(UUID().uuidString.lowercased())"
    }

    private func recordCoreStep(request: BridgeRequest, response: BridgeResponse) {
        guard let db = storage.databaseHandle,
              request.method == "core.step",
              response.ok,
              let event = request.params["event"],
              let result = response.result as? [String: Any]
        else { return }
        ensureRuntimeSession(request)
        let activeInstallId = BridgeBudgetQuarantine.activeInstallId(database: db, appId: request.context.appId)
        let sessionId = runtimeSessionId(request)
        let stateVersion = (result["stateVersion"] as? NSNumber)?.int64Value
        let actions = result["actions"] as? [[String: Any]] ?? []
        let records = coreRecordCoreEvent(
            request: request,
            sessionId: sessionId,
            installId: activeInstallId,
            event: event,
            stateVersion: stateVersion,
            actions: actions
        )
        let sql = """
        INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, records.eventId)
        bind(statement, 2, records.sessionId)
        bind(statement, 3, records.appId)
        bindNullable(statement, 4, records.installId)
        bindNullableInt(statement, 5, records.stateVersionBefore)
        bind(statement, 6, records.eventJSON)
        guard sqlite3_step(statement) == SQLITE_DONE else { return }
        for action in records.actions {
            recordCoreAction(
                actionId: action.actionId,
                eventId: records.eventId,
                sessionId: records.sessionId,
                appId: records.appId,
                actionJSON: action.actionJSON
            )
        }
    }

    private struct CoreEventInsertRecord {
        let eventId: String
        let sessionId: String
        let appId: String
        let installId: String?
        let stateVersionBefore: Int?
        let eventJSON: String
        let actions: [CoreActionInsertRecord]
    }

    private struct CoreActionInsertRecord {
        let actionId: String
        let actionJSON: String
    }

    private func coreRecordCoreEvent(
        request: BridgeRequest,
        sessionId: String,
        installId: String?,
        event: Any,
        stateVersion: Int64?,
        actions: [[String: Any]]
    ) -> CoreEventInsertRecord {
        if let payload = core.bridgeCommandDictionary(
            name: "bridge.record_core_event",
            payload: [
                "record": [
                    "platform_ids": ForgeCoreBridge.bridgePlatformIds(),
                    "session_id": sessionId,
                    "request_id": request.id ?? "unknown",
                    "app_id": request.context.appId,
                    "install_id": installId as Any,
                    "event": event,
                    "result_state_version": stateVersion as Any,
                    "actions": actions,
                ],
            ],
            requestId: request.id ?? "macos-record-core-event"
        ),
           let eventRecord = payload["event"] as? [String: Any] {
            let actionRecords = (payload["actions"] as? [[String: Any]] ?? []).map { action in
                CoreActionInsertRecord(
                    actionId: action["action_id"] as? String ?? "core_action_macos_\(UUID().uuidString.lowercased())",
                    actionJSON: jsonString(action["action_json"] ?? [:])
                )
            }
            return CoreEventInsertRecord(
                eventId: eventRecord["event_id"] as? String ?? "core_event_macos_\(UUID().uuidString.lowercased())",
                sessionId: eventRecord["session_id"] as? String ?? sessionId,
                appId: eventRecord["app_id"] as? String ?? request.context.appId,
                installId: eventRecord["install_id"] as? String ?? installId,
                stateVersionBefore: (eventRecord["state_version_before"] as? NSNumber)?.intValue,
                eventJSON: jsonString(eventRecord["event_json"] ?? event),
                actions: actionRecords
            )
        }
        return CoreEventInsertRecord(
            eventId: "core_event_macos_\(UUID().uuidString.lowercased())",
            sessionId: sessionId,
            appId: request.context.appId,
            installId: installId,
            stateVersionBefore: stateVersion.map { max(0, Int($0) - 1) },
            eventJSON: jsonString(event),
            actions: actions.enumerated().map { index, action in
                CoreActionInsertRecord(
                    actionId: "core_action_macos_\(UUID().uuidString.lowercased())_\(index)",
                    actionJSON: jsonString(action)
                )
            }
        )
    }

    private func recordCoreAction(actionId: String, eventId: String, sessionId: String, appId: String, actionJSON: String) {
        guard let db = storage.databaseHandle else { return }
        let sql = """
        INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at)
        VALUES (?, ?, ?, ?, ?, datetime('now'))
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, actionId)
        bind(statement, 2, eventId)
        bind(statement, 3, sessionId)
        bind(statement, 4, appId)
        bind(statement, 5, actionJSON)
        sqlite3_step(statement)
    }

    private func ensureRuntimeSession(_ request: BridgeRequest) {
        guard let db = storage.databaseHandle else { return }
        let activeInstallId = BridgeBudgetQuarantine.activeInstallId(database: db, appId: request.context.appId)
        let session = corePrepareSession(request: request)
        let sql = """
        INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json)
        VALUES (?, 'macos', 'macos', ?, ?, ?, datetime('now'), 'running', '{}', ?)
        ON CONFLICT(session_id) DO UPDATE SET active_app_id = excluded.active_app_id, active_install_id = excluded.active_install_id, status = 'running', metadata_json = excluded.metadata_json
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, session.sessionId)
        bind(statement, 2, ForgeDataCatalog.shared.runtimeVersion)
        bind(statement, 3, request.context.appId)
        bindNullable(statement, 4, activeInstallId)
        bind(statement, 5, session.metadataJSON)
        sqlite3_step(statement)
    }

    private struct RuntimeSessionInsert {
        let sessionId: String
        let metadataJSON: String
    }

    private func corePrepareSession(request: BridgeRequest) -> RuntimeSessionInsert {
        if let payload = core.bridgeCommandDictionary(
            name: "bridge.prepare_session",
            payload: [
                "platform_ids": ForgeCoreBridge.bridgePlatformIds(),
                "app_id": request.context.appId,
                "mount_token": request.context.mountToken ?? "native",
                "metadata": [
                    "source": "native-macos-bridge",
                    "reloadOffered": false,
                    "canAutoRemount": false,
                    "runtimeReady": false,
                ],
            ],
            requestId: "macos-prepare-session-\(request.context.appId)"
        ) {
            return RuntimeSessionInsert(
                sessionId: payload["session_id"] as? String ?? runtimeSessionId(request),
                metadataJSON: jsonString(payload["metadata"] as? [String: Any] ?? ["source": "native-macos-bridge"])
            )
        }
        return RuntimeSessionInsert(
            sessionId: runtimeSessionId(request),
            metadataJSON: jsonBody(["source": "native-macos-bridge"])
        )
    }

    private func runtimeSessionId(_ request: BridgeRequest) -> String {
        if let payload = core.bridgeCommandDictionary(
            name: "bridge.prepare_session",
            payload: [
                "platform_ids": ForgeCoreBridge.bridgePlatformIds(),
                "app_id": request.context.appId,
                "mount_token": request.context.mountToken ?? "native",
            ],
            requestId: "macos-session-id-\(request.context.appId)"
        ),
           let sessionId = payload["session_id"] as? String {
            return sessionId
        }
        return "runtime_macos_\(request.context.appId)_\(request.context.mountToken ?? "native")"
    }

    private func stateVersionBefore(_ result: [String: Any]) -> Int? {
        guard let value = result["stateVersion"] as? NSNumber else { return nil }
        return max(0, value.intValue - 1)
    }

    private func bind(_ statement: OpaquePointer?, _ index: Int32, _ value: String) {
        sqlite3_bind_text(statement, index, value, -1, SQLITE_TRANSIENT_BRIDGE)
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
}

struct BridgeEnvelope {
    let appId: String?
    let mountToken: String?
    let requestBody: [String: Any]
    let isRuntimeEnvelope: Bool
    private let hasRequestBody: Bool

    init(body: [String: Any]) {
        self.appId = body["appId"] as? String
        self.mountToken = body["mountToken"] as? String
        let request = body["request"] as? [String: Any]
        self.requestBody = request ?? body
        self.hasRequestBody = request != nil
        self.isRuntimeEnvelope = body["request"] != nil || body["mountToken"] != nil || body["appId"] != nil
    }

    var hasValidContext: Bool {
        appId?.isEmpty == false && mountToken?.isEmpty == false && hasRequestBody
    }

    var requestId: String? {
        requestBody["id"] as? String
    }
}

struct BridgeRequest {
    let id: String?
    let method: String
    let params: [String: Any]
    let context: AppSandboxContext

    init(id: String?, method: String, params: [String: Any], context: AppSandboxContext) {
        self.id = id
        self.method = method
        self.params = params
        self.context = context
    }

    init(body: [String: Any], context: AppSandboxContext) {
        self.id = body["id"] as? String
        self.method = body["method"] as! String
        self.params = body["params"] as! [String: Any]
        self.context = context
    }
}

extension BridgeRequest: @unchecked Sendable {}

struct AppSandboxContext {
    let appId: String
    let storagePrefix: String
    let approvedPermissions: Set<String>
    let networkPolicy: [NetworkPolicyRule]
    let networkPolicyPayload: [String: Any]
    let denyPrivateNetwork: Bool
    let resourceBudget: [String: Int]
    let resourceBudgetPayload: [String: Any]
    let mountToken: String?

    init(
        appId: String,
        storagePrefix: String? = nil,
        approvedPermissions: Set<String>,
        networkPolicy: [NetworkPolicyRule],
        networkPolicyPayload: [String: Any] = [:],
        denyPrivateNetwork: Bool,
        resourceBudget: [String: Int] = [:],
        resourceBudgetPayload: [String: Any] = [:],
        mountToken: String?
    ) {
        self.appId = appId
        self.storagePrefix = storagePrefix ?? "\(appId):"
        self.approvedPermissions = approvedPermissions
        self.networkPolicy = networkPolicy
        self.networkPolicyPayload = networkPolicyPayload
        self.denyPrivateNetwork = denyPrivateNetwork
        self.resourceBudget = resourceBudget
        self.resourceBudgetPayload = resourceBudgetPayload
        self.mountToken = mountToken
    }

    @MainActor
    init(message: WKScriptMessage, envelope: BridgeEnvelope, core: ForgeCoreBridge = ForgeCoreBridge()) {
        let envelopeAppId = message.frameInfo.isMainFrame ? envelope.appId : nil
        let appId = envelopeAppId ?? AppSandboxContext.appId(from: message.frameInfo.request.url) ?? "unknown"
        let manifest = AppSandboxContext.trustedManifest(for: appId)
        if let trusted = AppSandboxContext.permissionsFromCore(appId: appId, manifest: manifest, core: core) {
            self.appId = appId
            self.storagePrefix = trusted.storagePrefix
            self.approvedPermissions = trusted.approvedPermissions
            self.networkPolicy = trusted.networkPolicy
            self.networkPolicyPayload = trusted.networkPolicyPayload
            self.denyPrivateNetwork = trusted.denyPrivateNetwork
            self.resourceBudget = trusted.resourceBudget
            self.resourceBudgetPayload = trusted.resourceBudgetPayload
            self.mountToken = envelope.mountToken
            return
        }
        self.appId = appId
        self.storagePrefix = "\(appId):"
        self.approvedPermissions = AppSandboxContext.permissions(from: manifest)
        self.networkPolicy = NetworkPolicyRule.fromManifest(manifest)
        self.networkPolicyPayload = AppSandboxContext.networkPolicyPayload(from: manifest)
        self.denyPrivateNetwork = AppSandboxContext.denyPrivateNetwork(from: manifest)
        self.resourceBudget = AppSandboxContext.resourceBudget(from: manifest)
        self.resourceBudgetPayload = AppSandboxContext.resourceBudgetPayload(from: manifest)
        self.mountToken = envelope.mountToken
    }

    private static func appId(from url: URL?) -> String? {
        guard let path = url?.path else { return nil }
        let marker = "/webapps/examples/"
        guard let markerRange = path.range(of: marker) else { return nil }
        let rest = path[markerRange.upperBound...]
        guard let id = rest.split(separator: "/").first, !id.isEmpty else { return nil }
        return String(id)
    }

    private static func trustedManifest(for appId: String) -> [String: Any] {
        if let installed = installedManifest(for: appId) {
            return installed
        }
        return bundledManifest(for: appId)
    }

    private static func installedManifest(for appId: String) -> [String: Any]? {
        let database = PlatformDatabase()
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
        sqlite3_bind_text(statement, 1, appId, -1, SQLITE_TRANSIENT_BRIDGE)
        guard sqlite3_step(statement) == SQLITE_ROW,
              let text = sqlite3_column_text(statement, 0)
        else {
            return nil
        }
        let jsonText = String(cString: text)
        guard let data = jsonText.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return nil
        }
        return json
    }

    private static func bundledManifest(for appId: String) -> [String: Any] {
        guard let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return [:]
        }
        return json
    }

    private struct TrustedPermissions {
        let storagePrefix: String
        let approvedPermissions: Set<String>
        let networkPolicy: [NetworkPolicyRule]
        let networkPolicyPayload: [String: Any]
        let denyPrivateNetwork: Bool
        let resourceBudget: [String: Int]
        let resourceBudgetPayload: [String: Any]
    }

    private static func permissionsFromCore(
        appId: String,
        manifest: [String: Any],
        core: ForgeCoreBridge
    ) -> TrustedPermissions? {
        guard core.isAvailable, !manifest.isEmpty else { return nil }
        guard let payload = core.bridgeCommandDictionary(
            name: "package.get_permissions",
            payload: [
                "app_id": appId,
                "manifest_json": manifest,
            ],
            requestId: "macos-package-permissions-\(appId)"
        ) else {
            return nil
        }
        let permissions = payload["permissions"] as? [String] ?? []
        let storagePrefix = payload["storage_prefix"] as? String ?? "\(appId):"
        let networkPolicyPayload = payload["network_policy"] as? [String: Any] ?? [:]
        let denyPrivateNetwork = payload["deny_private_network"] as? Bool ?? true
        let resourceBudgetPayload = payload["resource_budget"] as? [String: Any] ?? [:]
        return TrustedPermissions(
            storagePrefix: storagePrefix,
            approvedPermissions: Set(permissions),
            networkPolicy: NetworkPolicyRule.fromManifest(["networkPolicy": networkPolicyPayload]),
            networkPolicyPayload: networkPolicyPayload,
            denyPrivateNetwork: denyPrivateNetwork,
            resourceBudget: resourceBudgetFromPayload(resourceBudgetPayload),
            resourceBudgetPayload: resourceBudgetPayload
        )
    }

    static func networkPolicyPayload(from manifest: [String: Any]) -> [String: Any] {
        manifest["networkPolicy"] as? [String: Any] ?? [:]
    }

    static func resourceBudgetPayload(from manifest: [String: Any]) -> [String: Any] {
        manifest["resourceBudget"] as? [String: Any] ?? [:]
    }

    private static func resourceBudgetFromPayload(_ payload: [String: Any]) -> [String: Int] {
        var normalized: [String: Int] = [:]
        let keyMap = [
            "maxBridgeCallsPerMinute": "max_bridge_calls_per_minute",
            "maxNetworkRequestsPerMinute": "max_network_requests_per_minute",
            "maxLogLinesPerMinute": "max_log_lines_per_minute",
            "maxNetworkResponseBytes": "max_network_response_bytes",
        ]
        for (camel, snake) in keyMap {
            if let value = intValue(payload[camel]) ?? intValue(payload[snake]) {
                normalized[camel] = value
            }
        }
        return normalized
    }

    private static func intValue(_ value: Any?) -> Int? {
        if let intValue = value as? Int { return intValue }
        if let number = value as? NSNumber { return number.intValue }
        return nil
    }

    private static func permissions(from manifest: [String: Any]) -> Set<String> {
        guard let permissions = manifest["permissions"] as? [String] else { return [] }
        return Set(permissions)
    }

    private static func denyPrivateNetwork(from manifest: [String: Any]) -> Bool {
        guard let policy = manifest["networkPolicy"] as? [String: Any] else { return true }
        return (policy["denyPrivateNetwork"] as? Bool) ?? true
    }

    static func resourceBudget(from manifest: [String: Any]) -> [String: Int] {
        guard let budget = manifest["resourceBudget"] as? [String: Any] else { return [:] }
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
}

extension AppSandboxContext: @unchecked Sendable {}

struct BridgeResponse {
    let id: String?
    let ok: Bool
    let result: Any?
    let error: [String: Any]?

    static func success(id: String?, result: Any) -> BridgeResponse {
        BridgeResponse(id: id, ok: true, result: result, error: nil)
    }

    static func failure(id: String?, code: String, message: String, details: [String: Any] = [:]) -> BridgeResponse {
        BridgeResponse(
            id: id,
            ok: false,
            result: nil,
            error: ["code": code, "message": message, "details": details]
        )
    }

    func asDictionary() -> [String: Any] {
        var body: [String: Any] = ["ok": ok]
        if let id {
            body["id"] = id
        }
        if let result {
            body["result"] = result
        }
        if let error {
            body["error"] = error
        }
        return body
    }
}

extension BridgeResponse: @unchecked Sendable {}

private let SQLITE_TRANSIENT_BRIDGE = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

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

private func jsonOptionalString(_ value: Any?) -> String? {
    guard let value, !(value is NSNull) else { return nil }
    return jsonString(value)
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
