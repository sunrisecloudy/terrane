package com.nativeai.platform

import android.content.Context
import org.json.JSONObject

class NativeBridge(
    context: Context,
    private val contextForApp: (String) -> AppSandboxContext,
) {
    private val storage = PlatformStorage(context)
    private val dialogs = PlatformDialogs()
    private val notifications = PlatformNotifications()
    private val network = PlatformNetwork()
    private val core = ZigCoreBridge()
    private val trustedRuntimeOrigin = "https://appassets.androidplatform.net"

    fun handleEnvelope(body: String, isMainFrame: Boolean, sourceOrigin: String): String {
        val envelope = try {
            JSONObject(body)
        } catch (error: Exception) {
            return BridgeResponse.failure(null, "invalid_request", "Runtime bridge envelope must be JSON").toString()
        }
        val requestBody = envelope.optJSONObject("request")
        val requestId = requestBody?.optString("id")?.ifBlank { null }

        if (!isMainFrame || sourceOrigin != trustedRuntimeOrigin) {
            return BridgeResponse.failure(
                requestId,
                "bridge.unauthorized_channel",
                "Runtime bridge envelope must come from the trusted main runtime frame",
            ).toString()
        }

        val appId = envelope.optString("appId").ifBlank { null }
        val mountToken = envelope.optString("mountToken").ifBlank { null }
        if (appId == null || mountToken == null || requestBody == null) {
            return BridgeResponse.failure(
                requestId,
                "invalid_request",
                "Runtime bridge envelope requires appId, mountToken, and request",
            ).toString()
        }

        val context = try {
            contextForApp(appId).copy(mountToken = mountToken)
        } catch (error: Exception) {
            return BridgeResponse.failure(requestId, "invalid_request", "Runtime bridge envelope references an unknown app").toString()
        }
        if (context.appId != appId) {
            return BridgeResponse.failure(requestId, "invalid_request", "Runtime bridge envelope appId does not match the manifest").toString()
        }

        val request = try {
            BridgeRequest(requestBody, context)
        } catch (error: Exception) {
            return BridgeResponse.failure(requestId, "invalid_request", "Bridge request body must be JSON").toString()
        }

        val permission = permissionForBridgeMethod(request.method)
        if (permission != null && !request.context.approvedPermissions.contains(permission)) {
            return BridgeResponse.failure(
                request.id,
                "permission_denied",
                "App ${request.context.appId} cannot call ${request.method}",
                JSONObject(mapOf("appId" to request.context.appId, "method" to request.method, "requiredPermission" to permission)),
            ).toString()
        }

        return when (request.method) {
            "storage.get" -> storage.get(request)
            "storage.set" -> storage.set(request)
            "storage.remove" -> storage.remove(request)
            "storage.list" -> storage.list(request)
            "dialog.openFile" -> dialogs.openFile(request)
            "dialog.saveFile" -> dialogs.saveFile(request)
            "notification.toast" -> notifications.toast(request)
            "network.request" -> network.request(request)
            "core.step" -> core.step(request)
            "runtime.capabilities" -> BridgeResponse.success(request.id, capabilities()).toString()
            "app.log" -> BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true))).toString()
            else -> BridgeResponse.failure(request.id, "unknown_method", "Unknown bridge method: ${request.method}").toString()
        }
    }

    private fun permissionForBridgeMethod(method: String): String? = when (method) {
        "storage.get", "storage.list" -> "storage.read"
        "storage.set", "storage.remove" -> "storage.write"
        "dialog.openFile", "dialog.saveFile", "notification.toast", "network.request", "core.step" -> method
        else -> null
    }

    private fun capabilities(): JSONObject = JSONObject(
        mapOf(
            "platform" to "android",
            "target" to "android",
            "runtimeVersion" to "0.1.0",
            "devMode" to true,
            "features" to JSONObject(
                mapOf(
                    "storage.get" to true,
                    "storage.set" to true,
                    "storage.remove" to true,
                    "storage.list" to true,
                    "dialog.openFile" to false,
                    "dialog.saveFile" to false,
                    "notification.toast" to true,
                    "network.request" to true,
                    "core.step" to core.isAvailable(),
                    "runtime.capabilities" to true,
                    "app.log" to true,
                ),
            ),
            "limits" to JSONObject(mapOf("maxPackageBytes" to 1_048_576, "maxFileBytes" to 524_288)),
        ),
    )
}

data class AppSandboxContext(
    val appId: String,
    val storagePrefix: String,
    val approvedPermissions: Set<String>,
    val networkPolicy: List<NetworkPolicyRule> = emptyList(),
    val mountToken: String? = null,
)

class BridgeRequest(body: JSONObject, val context: AppSandboxContext) {
    val id: String? = body.optString("id").ifBlank { null }
    val method: String = body.optString("method")
    val params: JSONObject = body.optJSONObject("params") ?: JSONObject()
}

object BridgeResponse {
    fun success(id: String?, result: JSONObject): JSONObject {
        val body = JSONObject(mapOf("ok" to true, "result" to result))
        if (id != null) body.put("id", id)
        return body
    }

    fun failure(id: String?, code: String, message: String, details: JSONObject = JSONObject()): JSONObject {
        val body = JSONObject(
            mapOf(
                "ok" to false,
                "error" to JSONObject(mapOf("code" to code, "message" to message, "details" to details)),
            ),
        )
        if (id != null) body.put("id", id)
        return body
    }
}
