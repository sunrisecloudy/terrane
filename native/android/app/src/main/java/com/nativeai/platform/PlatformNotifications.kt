package com.nativeai.platform

import org.json.JSONObject

class PlatformNotifications {
    fun toast(request: BridgeRequest): String {
        val message = request.params.opt("message")
        if (message !is String) {
            return BridgeResponse.failure(request.id, "invalid_request", "notification.toast requires message").toString()
        }
        val level = request.params.opt("level")
        if (level != null && level != JSONObject.NULL) {
            if (level !is String) {
                return BridgeResponse.failure(request.id, "invalid_request", "notification.toast level must be a string").toString()
            }
            if (level !in setOf("info", "success", "warning", "error")) {
                return BridgeResponse.failure(
                    request.id,
                    "invalid_request",
                    "notification.toast level must be info, success, warning, or error",
                    JSONObject(mapOf("level" to level)),
                ).toString()
            }
        }
        return BridgeResponse.success(
            request.id,
            JSONObject(mapOf("ok" to true)),
        ).toString()
    }
}
