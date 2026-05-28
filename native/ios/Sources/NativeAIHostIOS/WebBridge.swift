import Foundation
import WebKit

@MainActor
final class WebBridge: NSObject, WKScriptMessageHandlerWithReply {
    private let storage = PlatformStorage()
    private let dialogs = PlatformDialogs()
    private let notifications = PlatformNotifications()
    private let network = PlatformNetwork()
    private let core = ZigCoreBridge()

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any] else {
            replyHandler(BridgeResponse.failure(id: nil, code: "invalid_request", message: "Bridge message body must be an object").asDictionary(), nil)
            return
        }

        let request = BridgeRequest(body: body, context: AppSandboxContext(message: message))
        if let permission = permissionForBridgeMethod(request.method),
           !request.context.approvedPermissions.contains(permission) {
            replyHandler(
                BridgeResponse.failure(
                    id: request.id,
                    code: "permission_denied",
                    message: "App \(request.context.appId) cannot call \(request.method)",
                    details: ["appId": request.context.appId, "method": request.method, "requiredPermission": permission]
                ).asDictionary(),
                nil
            )
            return
        }

        replyHandler(dispatch(request).asDictionary(), nil)
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
            return .success(id: request.id, result: [
                "platform": "ios",
                "target": "ios-simulator",
                "runtimeVersion": "0.1.0",
                "devMode": true,
                "features": [
                    "storage.get": true,
                    "storage.set": true,
                    "storage.remove": true,
                    "storage.list": true,
                    "dialog.openFile": false,
                    "dialog.saveFile": false,
                    "notification.toast": true,
                    "network.request": false,
                    "core.step": false,
                    "runtime.capabilities": true,
                    "app.log": true
                ],
                "limits": [
                    "maxPackageBytes": 1_048_576,
                    "maxFileBytes": 524_288
                ]
            ])
        case "app.log":
            NSLog("Generated app log: \(request.params)")
            return .success(id: request.id, result: ["ok": true])
        default:
            return .failure(id: request.id, code: "unknown_method", message: "Unknown bridge method: \(request.method)")
        }
    }

    private func permissionForBridgeMethod(_ method: String) -> String? {
        switch method {
        case "storage.get", "storage.list":
            return "storage.read"
        case "storage.set", "storage.remove":
            return "storage.write"
        case "dialog.openFile", "dialog.saveFile", "notification.toast", "network.request", "core.step", "app.log":
            return method
        default:
            return nil
        }
    }
}

struct BridgeRequest {
    let id: String?
    let method: String
    let params: [String: Any]
    let context: AppSandboxContext

    init(body: [String: Any], context: AppSandboxContext) {
        self.id = body["id"] as? String
        self.method = body["method"] as? String ?? ""
        self.params = body["params"] as? [String: Any] ?? [:]
        self.context = context
    }
}

struct AppSandboxContext {
    let appId: String
    let storagePrefix: String
    let approvedPermissions: Set<String>

    @MainActor
    init(message: WKScriptMessage) {
        let appId = AppSandboxContext.appId(from: message.frameInfo.request.url) ?? "unknown"
        self.appId = appId
        self.storagePrefix = "\(appId):"
        self.approvedPermissions = AppSandboxContext.permissions(for: appId)
    }

    private static func appId(from url: URL?) -> String? {
        guard let path = url?.path else { return nil }
        for marker in ["/webapps/examples/", "/examples/"] {
            guard let markerRange = path.range(of: marker) else { continue }
            let rest = path[markerRange.upperBound...]
            guard let id = rest.split(separator: "/").first, !id.isEmpty else { continue }
            return String(id)
        }
        return nil
    }

    private static func permissions(for appId: String) -> Set<String> {
        guard let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let permissions = json["permissions"] as? [String]
        else {
            return []
        }
        return Set(permissions)
    }
}

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
