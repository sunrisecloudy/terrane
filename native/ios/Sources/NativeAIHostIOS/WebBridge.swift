import Foundation
import UIKit
import WebKit

typealias BridgeReply = @MainActor @Sendable (BridgeResponse) -> Void

@MainActor
final class WebBridge: NSObject, WKScriptMessageHandlerWithReply {
    private let storage = PlatformStorage()
    private let dialogs = PlatformDialogs()
    private let notifications = PlatformNotifications()
    private let network = PlatformNetwork()
    private let core = ZigCoreBridge()

    func setDialogPresenterProvider(_ provider: @escaping @MainActor () -> UIViewController?) {
        dialogs.presenterProvider = provider
    }

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

        let request = BridgeRequest(body: envelope.requestBody, context: AppSandboxContext(message: message, envelope: envelope))
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

        dispatch(request) { response in
            replyHandler(response.asDictionary(), nil)
        }
    }

    private func dispatch(_ request: BridgeRequest, reply: @escaping BridgeReply) {
        switch request.method {
        case "storage.get":
            reply(storage.get(request))
        case "storage.set":
            reply(storage.set(request))
        case "storage.remove":
            reply(storage.remove(request))
        case "storage.list":
            reply(storage.list(request))
        case "dialog.openFile":
            dialogs.openFile(request, reply: reply)
        case "dialog.saveFile":
            dialogs.saveFile(request, reply: reply)
        case "notification.toast":
            reply(notifications.toast(request))
        case "network.request":
            reply(network.request(request))
        case "core.step":
            reply(core.step(request))
        case "runtime.capabilities":
            reply(.success(id: request.id, result: [
                "platform": "ios",
                "target": "ios-simulator",
                "runtimeVersion": "0.1.0",
                "devMode": true,
                "features": [
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
                    "app.log": true
                ],
                "limits": [
                    "maxPackageBytes": 1_048_576,
                    "maxFileBytes": 524_288
                ]
            ]))
        case "app.log":
            NSLog("Generated app log: \(request.params)")
            reply(.success(id: request.id, result: ["ok": true]))
        default:
            reply(.failure(id: request.id, code: "unknown_method", message: "Unknown bridge method: \(request.method)"))
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
    let networkPolicy: [NetworkPolicyRule]
    let mountToken: String?

    @MainActor
    init(message: WKScriptMessage, envelope: BridgeEnvelope) {
        let envelopeAppId = message.frameInfo.isMainFrame ? envelope.appId : nil
        let appId = envelopeAppId ?? AppSandboxContext.appId(from: message.frameInfo.request.url) ?? "unknown"
        let manifest = AppSandboxContext.manifest(for: appId)
        self.appId = appId
        self.storagePrefix = "\(appId):"
        self.approvedPermissions = AppSandboxContext.permissions(from: manifest)
        self.networkPolicy = NetworkPolicyRule.fromManifest(manifest)
        self.mountToken = envelope.mountToken
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
