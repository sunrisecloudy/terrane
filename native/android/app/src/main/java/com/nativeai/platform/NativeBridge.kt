package com.nativeai.platform

import android.content.Context
import org.json.JSONObject

class NativeBridge(
    context: Context,
    private val dialogs: PlatformDialogs,
    private val contextForApp: (String) -> AppSandboxContext,
) {
    private val storage = PlatformStorage(context)
    private val notifications = PlatformNotifications()
    private val network = PlatformNetwork()
    private val core = ZigCoreBridge()
    private val trustedRuntimeOrigin = "https://appassets.androidplatform.net"

    fun handleEnvelope(body: String, isMainFrame: Boolean, sourceOrigin: String, respond: (String) -> Unit) {
        val envelope = try {
            JSONObject(body)
        } catch (error: Exception) {
            respond(BridgeResponse.failure(null, "invalid_request", "Runtime bridge envelope must be JSON").toString())
            return
        }
        val requestBody = envelope.optJSONObject("request")
        val requestId = requestBody?.optString("id")?.ifBlank { null }

        if (!isMainFrame || sourceOrigin != trustedRuntimeOrigin) {
            respond(BridgeResponse.failure(
                requestId,
                "bridge.unauthorized_channel",
                "Runtime bridge envelope must come from the trusted main runtime frame",
            ).toString())
            return
        }

        val appId = envelope.optString("appId").ifBlank { null }
        val mountToken = envelope.optString("mountToken").ifBlank { null }
        if (appId == null || mountToken == null || requestBody == null) {
            respond(BridgeResponse.failure(
                requestId,
                "invalid_request",
                "Runtime bridge envelope requires appId, mountToken, and request",
            ).toString())
            return
        }

        val context = try {
            contextForApp(appId).copy(mountToken = mountToken)
        } catch (error: Exception) {
            respond(BridgeResponse.failure(requestId, "invalid_request", "Runtime bridge envelope references an unknown app").toString())
            return
        }
        if (context.appId != appId) {
            respond(BridgeResponse.failure(requestId, "invalid_request", "Runtime bridge envelope appId does not match the manifest").toString())
            return
        }

        val request = try {
            BridgeRequest(requestBody, context)
        } catch (error: Exception) {
            respond(BridgeResponse.failure(requestId, "invalid_request", "Bridge request body must be JSON").toString())
            return
        }

        val permission = permissionForBridgeMethod(request.method)
        if (permission != null && !request.context.approvedPermissions.contains(permission)) {
            respond(BridgeResponse.failure(
                request.id,
                "permission_denied",
                "App ${request.context.appId} cannot call ${request.method}",
                JSONObject(mapOf("appId" to request.context.appId, "method" to request.method, "requiredPermission" to permission)),
            ).toString())
            return
        }

        when (request.method) {
            "storage.get" -> respond(storage.get(request))
            "storage.set" -> respond(storage.set(request))
            "storage.remove" -> respond(storage.remove(request))
            "storage.list" -> respond(storage.list(request))
            "dialog.openFile" -> dialogs.openFile(request, respond)
            "dialog.saveFile" -> dialogs.saveFile(request, respond)
            "notification.toast" -> respond(notifications.toast(request))
            "network.request" -> respond(network.request(request))
            "core.step" -> respond(core.step(request))
            "runtime.capabilities" -> respond(BridgeResponse.success(request.id, capabilities()).toString())
            "app.log" -> respond(BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true))).toString())
            else -> respond(BridgeResponse.failure(request.id, "unknown_method", "Unknown bridge method: ${request.method}").toString())
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
                    "dialog.openFile" to true,
                    "dialog.saveFile" to true,
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
