import Foundation

final class ZigCoreBridge {
    func step(_ request: BridgeRequest) -> BridgeResponse {
        .failure(
            id: request.id,
            code: "platform_unsupported",
            message: "core.step requires linking libzig_core into the macOS target"
        )
    }
}
