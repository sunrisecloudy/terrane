import Foundation

final class PlatformNotifications {
    func toast(_ request: BridgeRequest) -> BridgeResponse {
        NSLog("Toast: \(request.params["message"] ?? "")")
        return .success(id: request.id, result: ["ok": true])
    }
}
