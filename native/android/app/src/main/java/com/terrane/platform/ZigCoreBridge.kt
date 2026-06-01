package com.terrane.platform

import android.util.Log
import org.json.JSONObject

class ZigCoreBridge {
    fun isAvailable(): Boolean = zigCoreLoaded && jniLoaded && runCatching { nativeIsAvailable() }.getOrDefault(false)

    fun step(request: BridgeRequest): String {
        if (!isAvailable()) {
            return BridgeResponse.failure(
                request.id,
                "platform_unsupported",
                "core.step requires JNI libzig_core for Android ABIs",
            ).toString()
        }

        if (request.params.has("app") && !request.params.isNull("app")) {
            val requestedApp = request.params.opt("app")
            if (requestedApp !is String) {
                return BridgeResponse.failure(
                    request.id,
                    "invalid_request",
                    "core.step app field must be a string when present",
                ).toString()
            }
            if (requestedApp != request.context.appId) {
                return BridgeResponse.failure(
                    request.id,
                    "permission_denied",
                    "core.step app field does not match the channel-derived app id",
                    JSONObject(mapOf("requestedApp" to requestedApp, "channelApp" to request.context.appId)),
                ).toString()
            }
        }

        val input = JSONObject(request.params.toString())
        input.put("app", request.context.appId)

        val output = runCatching { nativeStep(input.toString()) }.getOrNull()
            ?: return BridgeResponse.failure(request.id, "core_error", "core_step_json failed").toString()

        val result = runCatching { JSONObject(output) }.getOrNull()
            ?: return BridgeResponse.failure(request.id, "core_error", "core.step returned invalid JSON").toString()
        return BridgeResponse.success(request.id, result).toString()
    }

    private external fun nativeIsAvailable(): Boolean
    private external fun nativeStep(inputJson: String): String?

    companion object {
        private val zigCoreLoaded: Boolean = runCatching {
            System.loadLibrary("zig_core")
            true
        }.onFailure { error ->
            Log.e("TerranePlatformCore", "System.loadLibrary(\"zig_core\") failed", error)
        }.getOrDefault(false)

        private val jniLoaded: Boolean = runCatching {
            System.loadLibrary("zig_core_jni")
            true
        }.onFailure { error ->
            Log.e("TerranePlatformCore", "System.loadLibrary(\"zig_core_jni\") failed", error)
        }.getOrDefault(false)
    }
}
