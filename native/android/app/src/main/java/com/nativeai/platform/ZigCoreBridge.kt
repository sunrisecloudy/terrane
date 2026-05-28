package com.nativeai.platform

class ZigCoreBridge {
    fun step(request: BridgeRequest): String = BridgeResponse.failure(
        request.id,
        "platform_unsupported",
        "core.step requires JNI libzig_core for Android ABIs",
    ).toString()
}
