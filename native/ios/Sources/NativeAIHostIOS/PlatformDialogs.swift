import Foundation

@MainActor
final class PlatformDialogs {
    func openFile(_ request: BridgeRequest) -> BridgeResponse {
        .failure(id: request.id, code: "platform_unsupported", message: "dialog.openFile is not available in the iOS host yet")
    }

    func saveFile(_ request: BridgeRequest) -> BridgeResponse {
        .failure(id: request.id, code: "platform_unsupported", message: "dialog.saveFile is not available in the iOS host yet")
    }
}
