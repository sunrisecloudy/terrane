import Foundation
import WebKit

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
            replyHandler(BridgeResponse.failure(id: nil, code: "invalid_request", message: "Bridge message body must be an object"), nil)
            return
        }

        let request = BridgeRequest(body: body)
        let result = dispatch(request)
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
            return .success(id: request.id, result: [
                "platform": "macos",
                "target": "macos",
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
}

struct BridgeRequest {
    let id: String?
    let method: String
    let params: [String: Any]

    init(body: [String: Any]) {
        self.id = body["id"] as? String
        self.method = body["method"] as? String ?? ""
        self.params = body["params"] as? [String: Any] ?? [:]
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
