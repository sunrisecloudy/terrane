import Foundation

final class PlatformNotifications {
    func toast(_ request: BridgeRequest) -> BridgeResponse {
        guard let message = request.params["message"] as? String else {
            return .failure(id: request.id, code: "invalid_request", message: "notification.toast requires message")
        }
        if let levelValue = request.params["level"], !(levelValue is NSNull) {
            guard let level = levelValue as? String else {
                return .failure(id: request.id, code: "invalid_request", message: "notification.toast level must be a string")
            }
            guard Self.validNotificationLevel(level) else {
                return .failure(
                    id: request.id,
                    code: "invalid_request",
                    message: "notification.toast level must be info, success, warning, or error",
                    details: ["level": level]
                )
            }
        }
        NSLog("Toast: \(message)")
        return .success(id: request.id, result: ["ok": true])
    }

    private static func validNotificationLevel(_ level: String) -> Bool {
        ["info", "success", "warning", "error"].contains(level)
    }
}
