package com.nativeai.platform

import org.json.JSONObject

class PlatformNotifications {
    fun toast(request: BridgeRequest): String = BridgeResponse.success(
        request.id,
        JSONObject(mapOf("ok" to true)),
    ).toString()
}
