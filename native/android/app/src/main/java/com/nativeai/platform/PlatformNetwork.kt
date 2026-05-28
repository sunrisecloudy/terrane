package com.nativeai.platform

class PlatformNetwork {
    fun request(request: BridgeRequest): String = BridgeResponse.failure(
        request.id,
        "platform_unsupported",
        "network.request will be wired through OkHttp/URLSession equivalent after manifest networkPolicy enforcement lands",
    ).toString()
}
