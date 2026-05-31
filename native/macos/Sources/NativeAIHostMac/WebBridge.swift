import Foundation
import SQLite3
import WebKit

@MainActor
final class WebBridge: NSObject, WKScriptMessageHandlerWithReply {
    private let storage = PlatformStorage()
    private let dialogs = PlatformDialogs()
    private let notifications = PlatformNotifications()
    private let network = PlatformNetwork()
    private let core = ZigCoreBridge()
    private let crdt = ZigCrdtBridge()
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
        if envelope.isRuntimeEnvelope && !message.frameInfo.isMainFrame {
            replyHandler(
                BridgeResponse.failure(
                    id: envelope.requestId,
                    code: "bridge.unauthorized_channel",
                    message: "Runtime bridge envelope must come from the main runtime frame"
                ).asDictionary(),
                nil
            )
            return
        }
        if envelope.isRuntimeEnvelope && !Self.hasOnlyRuntimeEnvelopeFields(body) {
            replyHandler(
                BridgeResponse.failure(
                    id: envelope.requestId,
                    code: "invalid_request",
                    message: "Runtime bridge envelope contains unknown top-level fields",
                    details: ["fields": Self.extraFields(in: body, allowed: Self.runtimeEnvelopeFields)]
                ).asDictionary(),
                nil
            )
            return
        }
        if envelope.isRuntimeEnvelope && !envelope.hasValidContext {
            replyHandler(
                BridgeResponse.failure(
                    id: envelope.requestId,
                    code: "invalid_request",
                    message: "Runtime bridge envelope requires appId, mountToken, and request"
                ).asDictionary(),
                nil
            )
            return
        }
        if let validationFailure = Self.bridgeRequestValidationFailure(envelope.requestBody) {
            replyHandler(validationFailure.asDictionary(), nil)
            return
        }

        let request = BridgeRequest(body: envelope.requestBody, context: AppSandboxContext(message: message, envelope: envelope))
        let startedAt = Date()
        if request.params["appId"] != nil {
            let response = BridgeResponse.failure(
                id: request.id,
                code: "invalid_request",
                message: "Bridge params must not include appId; app id is channel-derived",
                details: ["field": "appId"]
            )
            recordBridgeCall(request: request, response: response, startedAt: startedAt)
            replyHandler(response.asDictionary(), nil)
            return
        }
        if let permission = permissionForBridgeMethod(request.method),
           !request.context.approvedPermissions.contains(permission) {
            let response = BridgeResponse.failure(
                id: request.id,
                code: "permission_denied",
                message: "App \(request.context.appId) cannot call \(request.method)",
                details: ["appId": request.context.appId, "method": request.method, "requiredPermission": permission]
            )
            recordBridgeCall(request: request, response: response, startedAt: startedAt)
            replyHandler(
                response.asDictionary(),
                nil
            )
            return
        }
        if let response = bridgeRateBudgetFailure(request) {
            recordBridgeCall(request: request, response: response, startedAt: startedAt)
            replyHandler(response.asDictionary(), nil)
            return
        }
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
            var limits: [String: Any] = [
                "maxPackageBytes": 1_048_576,
                "maxFileBytes": 524_288
            ]
            for (key, value) in request.context.resourceBudget {
                limits[key] = value
            }
            return .success(id: request.id, result: [
                "platform": "macos",
                "target": "macos",
                "appId": request.context.appId,
                "runtimeVersion": "0.1.0",
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
                    "notebook.crdt": crdt.smokeMaterialize(),
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
        let sql = """
        INSERT INTO bridge_calls (bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
        """
        do {
            var statement: OpaquePointer?
            guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
            defer { sqlite3_finalize(statement) }
            bind(statement, 1, "bridge_macos_\(UUID().uuidString.lowercased())")
            bind(statement, 2, runtimeSessionId(request))
            bind(statement, 3, request.context.appId)
            bindNullable(statement, 4, activeInstallId)
            bind(statement, 5, request.method)
            bind(statement, 6, jsonString(request.params))
            bindNullable(statement, 7, response.result.map(jsonString))
            bindNullable(statement, 8, response.error.map(jsonString))
            sqlite3_bind_int64(statement, 9, Int64(Date().timeIntervalSince(startedAt) * 1000))
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

    private func recordCoreStep(request: BridgeRequest, response: BridgeResponse) {
        guard let db = storage.databaseHandle,
              request.method == "core.step",
              response.ok,
              let event = request.params["event"],
              let result = response.result as? [String: Any]
        else { return }
        ensureRuntimeSession(request)
        let activeInstallId = BridgeBudgetQuarantine.activeInstallId(database: db, appId: request.context.appId)
        let eventId = "core_event_macos_\(UUID().uuidString.lowercased())"
        let sql = """
        INSERT INTO core_events (event_id, session_id, app_id, install_id, state_version_before, event_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, eventId)
        bind(statement, 2, runtimeSessionId(request))
        bind(statement, 3, request.context.appId)
        bindNullable(statement, 4, activeInstallId)
        bindNullableInt(statement, 5, stateVersionBefore(result))
        bind(statement, 6, jsonString(event))
        guard sqlite3_step(statement) == SQLITE_DONE else { return }
        for action in result["actions"] as? [[String: Any]] ?? [] {
            recordCoreAction(eventId: eventId, sessionId: runtimeSessionId(request), appId: request.context.appId, action: action)
        }
    }

    private func recordCoreAction(eventId: String, sessionId: String, appId: String, action: [String: Any]) {
        guard let db = storage.databaseHandle else { return }
        let sql = """
        INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at)
        VALUES (?, ?, ?, ?, ?, datetime('now'))
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, "core_action_macos_\(UUID().uuidString.lowercased())")
        bind(statement, 2, eventId)
        bind(statement, 3, sessionId)
        bind(statement, 4, appId)
        bind(statement, 5, jsonString(action))
        sqlite3_step(statement)
    }

    private func ensureRuntimeSession(_ request: BridgeRequest) {
        guard let db = storage.databaseHandle else { return }
        let activeInstallId = BridgeBudgetQuarantine.activeInstallId(database: db, appId: request.context.appId)
        let sql = """
        INSERT INTO runtime_sessions (session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json)
        VALUES (?, 'macos', 'macos', '0.1.0', ?, ?, datetime('now'), 'running', '{}', '{"source":"native-macos-bridge"}')
        ON CONFLICT(session_id) DO UPDATE SET active_app_id = excluded.active_app_id, active_install_id = excluded.active_install_id, status = 'running'
        """
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(statement) }
        bind(statement, 1, runtimeSessionId(request))
        bind(statement, 2, request.context.appId)
        bindNullable(statement, 3, activeInstallId)
        sqlite3_step(statement)
    }

    private func runtimeSessionId(_ request: BridgeRequest) -> String {
        "runtime_macos_\(request.context.appId)_\(request.context.mountToken ?? "native")"
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
    let denyPrivateNetwork: Bool
    let resourceBudget: [String: Int]
    let mountToken: String?

    init(
        appId: String,
        storagePrefix: String? = nil,
        approvedPermissions: Set<String>,
        networkPolicy: [NetworkPolicyRule],
        denyPrivateNetwork: Bool,
        resourceBudget: [String: Int] = [:],
        mountToken: String?
    ) {
        self.appId = appId
        self.storagePrefix = storagePrefix ?? "\(appId):"
        self.approvedPermissions = approvedPermissions
        self.networkPolicy = networkPolicy
        self.denyPrivateNetwork = denyPrivateNetwork
        self.resourceBudget = resourceBudget
        self.mountToken = mountToken
    }

    @MainActor
    init(message: WKScriptMessage, envelope: BridgeEnvelope) {
        let envelopeAppId = message.frameInfo.isMainFrame ? envelope.appId : nil
        let appId = envelopeAppId ?? AppSandboxContext.appId(from: message.frameInfo.request.url) ?? "unknown"
        let manifest = AppSandboxContext.manifest(for: appId)
        self.appId = appId
        self.storagePrefix = "\(appId):"
        self.approvedPermissions = AppSandboxContext.permissions(from: manifest)
        self.networkPolicy = NetworkPolicyRule.fromManifest(manifest)
        self.denyPrivateNetwork = AppSandboxContext.denyPrivateNetwork(from: manifest)
        self.resourceBudget = AppSandboxContext.resourceBudget(from: manifest)
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

    private static func manifest(for appId: String) -> [String: Any] {
        guard let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return [:]
        }
        return json
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

private func jsonBody(_ object: [String: Any]) -> String {
    guard JSONSerialization.isValidJSONObject(object),
          let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
          let string = String(data: data, encoding: .utf8)
    else {
        return "{}"
    }
    return string
}
