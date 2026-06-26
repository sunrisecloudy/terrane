package com.terrane.platform

import android.content.Context
import android.util.Log
import org.json.JSONObject

class ForgeCoreBridge(context: Context) {
    private val databasePath: String = context.getDatabasePath("forge-workspace.sqlite")
        .also { it.parentFile?.mkdirs() }
        .absolutePath

    fun isAvailable(): Boolean = forgeFfiLoaded && jniLoaded && runCatching { nativeIsAvailable(databasePath) }.getOrDefault(false)

    fun step(request: BridgeRequest): String {
        if (!isAvailable()) {
            return BridgeResponse.failure(
                request.id,
                "platform_unsupported",
                "core.step requires JNI libforge_ffi for Android ABIs",
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

        val payload = JSONObject(request.params.toString())
        payload.put("app", request.context.appId)

        val command = commandEnvelope(
            requestId = request.id ?: "android-core-step",
            name = "legacy.core_step",
            payload = payload,
        )

        val output = runCatching { nativeHandleCommand(databasePath, command.toString()) }.getOrNull()
            ?: return BridgeResponse.failure(request.id, "core_error", "forge_core_handle_command failed").toString()

        val response = runCatching { JSONObject(output) }.getOrNull()
            ?: return BridgeResponse.failure(request.id, "core_error", "core.step returned invalid JSON").toString()
        if (!response.optBoolean("ok", false)) {
            return BridgeResponse.failure(
                request.id,
                "core_error",
                "legacy.core_step failed",
                JSONObject(mapOf("response" to response)),
            ).toString()
        }
        val result = response.optJSONObject("payload")
            ?: return BridgeResponse.failure(request.id, "core_error", "legacy.core_step returned non-object payload").toString()
        return BridgeResponse.success(request.id, result).toString()
    }

    fun controlCommand(name: String, payload: JSONObject): JSONObject? =
        bridgeCommandDictionary(name, payload, "android-control-${System.nanoTime()}")

    fun bridgeCommandDictionary(
        name: String,
        payload: JSONObject,
        requestId: String = "android-bridge-${System.nanoTime()}",
    ): JSONObject? {
        if (!isAvailable()) return null
        val command = commandEnvelope(requestId = requestId, name = name, payload = payload)
        val output = runCatching { nativeHandleCommand(databasePath, command.toString()) }.getOrNull() ?: return null
        val response = runCatching { JSONObject(output) }.getOrNull() ?: return null
        if (!response.optBoolean("ok", false)) return null
        return response.optJSONObject("payload")
    }

    fun bridgePlatformIds(): JSONObject =
        JSONObject(mapOf("platform" to "android", "target" to "android"))

    private fun commandEnvelope(requestId: String, name: String, payload: JSONObject): JSONObject =
        JSONObject()
            .put("request_id", requestId)
            .put("actor", JSONObject().put("actor", "android-host").put("role", "owner"))
            .put("workspace_id", "android-native")
            .put("name", name)
            .put("payload", payload)

    private external fun nativeIsAvailable(databasePath: String): Boolean
    private external fun nativeHandleCommand(databasePath: String, commandJson: String): String?

    companion object {
        private val forgeFfiLoaded: Boolean = runCatching {
            System.loadLibrary("forge_ffi")
            true
        }.onFailure { error ->
            Log.e("TerranePlatformCore", "System.loadLibrary(\"forge_ffi\") failed", error)
        }.getOrDefault(false)

        private val jniLoaded: Boolean = runCatching {
            System.loadLibrary("forge_core_jni")
            true
        }.onFailure { error ->
            Log.e("TerranePlatformCore", "System.loadLibrary(\"forge_core_jni\") failed", error)
        }.getOrDefault(false)
    }
}