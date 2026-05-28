import Foundation

final class PlatformNetwork {
    func request(_ request: BridgeRequest) -> BridgeResponse {
        .failure(
            id: request.id,
            code: "platform_unsupported",
            message: "network.request will be wired through URLSession after manifest networkPolicy enforcement lands"
        )
    }
}
