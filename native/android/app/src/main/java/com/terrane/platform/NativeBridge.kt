package com.terrane.platform

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.os.SystemClock
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.time.Instant
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class NativeBridge(
    context: Context,
    private val dialogs: PlatformDialogs,
    private val contextForApp: (String) -> AppSandboxContext,
) {
    private val database = PlatformDatabase(context)
    private val storage = PlatformStorage(context)
    private val notifications = PlatformNotifications()
    private val network = PlatformNetwork(database)
    private val core = ForgeCoreBridge()
    private val trustedRuntimeOrigin = "https://appassets.androidplatform.net"
    private val runtimeEnvelopeFields = setOf("appId", "mountToken", "request")
    private val bridgeRequestFields = setOf("id", "method", "params", "timestamp")

    fun handleControlBridgeCall(appId: String, method: String, params: JSONObject, id: String = "control_${UUID.randomUUID()}"): JSONObject {
        var responseText: String? = null
        val latch = CountDownLatch(1)
        val envelope = JSONObject(
            mapOf(
                "appId" to appId,
                "mountToken" to "android-dev-control",
                "request" to JSONObject(mapOf("id" to id, "method" to method, "params" to params)),
            ),
        )
        handleEnvelope(
            body = envelope.toString(),
            isMainFrame = true,
            sourceOrigin = trustedRuntimeOrigin,
        ) { response ->
            responseText = response
            latch.countDown()
        }
        if (!latch.await(10, TimeUnit.SECONDS)) {
            return BridgeResponse.failure(id, "timeout", "Timed out waiting for native bridge response")
        }
        return try {
            JSONObject(requireNotNull(responseText))
        } catch (error: Exception) {
            BridgeResponse.failure(id, "invalid_response", "Native bridge returned invalid JSON")
        }
    }

    fun handleEnvelope(body: String, isMainFrame: Boolean, sourceOrigin: String, respond: (String) -> Unit) {
        val envelope = try {
            JSONObject(body)
        } catch (error: Exception) {
            respond(BridgeResponse.failure(null, "invalid_request", "Runtime bridge envelope must be JSON").toString())
            return
        }
        val requestBody = envelope.optJSONObject("request")
        val requestId = (requestBody?.opt("id") as? String)?.ifBlank { null }

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

        if (!hasOnlyRuntimeEnvelopeFields(envelope)) {
            respond(BridgeResponse.failure(
                requestId,
                "invalid_request",
                "Runtime bridge envelope contains unknown top-level fields",
                JSONObject(mapOf("fields" to unknownFields(envelope, runtimeEnvelopeFields))),
            ).toString())
            return
        }

        val requestValidationFailure = validateBridgeRequest(requestBody)
        if (requestValidationFailure != null) {
            respond(requestValidationFailure)
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
        val startedAtMs = SystemClock.elapsedRealtime()
        fun respondWithLog(responseText: String) {
            recordBridgeCall(request, responseText, startedAtMs)
            recordCoreStep(request, responseText)
            respond(responseText)
        }

        if (request.params.has("appId")) {
            respondWithLog(BridgeResponse.failure(
                request.id,
                "invalid_request",
                "Bridge params must not include appId; app id is channel-derived",
                JSONObject(mapOf("field" to "appId")),
            ).toString())
            return
        }

        val faultResponse = faultInjectionFailure(request)
        if (faultResponse != null) {
            respondWithLog(faultResponse)
            return
        }

        val permission = permissionForBridgeMethod(request.method)
        if (permission != null && !request.context.approvedPermissions.contains(permission)) {
            respondWithLog(BridgeResponse.failure(
                request.id,
                "permission_denied",
                "App ${request.context.appId} cannot call ${request.method}",
                JSONObject(mapOf("appId" to request.context.appId, "method" to request.method, "requiredPermission" to permission)),
            ).toString())
            return
        }
        val budgetFailure = bridgeRateBudgetFailure(request)
        if (budgetFailure != null) {
            respondWithLog(budgetFailure)
            return
        }

        when (request.method) {
            "storage.get" -> respondWithLog(storage.get(request))
            "storage.set" -> respondWithLog(storage.set(request))
            "storage.remove" -> respondWithLog(storage.remove(request))
            "storage.list" -> respondWithLog(storage.list(request))
            "dialog.openFile" -> dialogs.openFile(request) { response -> respondWithLog(response) }
            "dialog.saveFile" -> dialogs.saveFile(request) { response -> respondWithLog(response) }
            "notification.toast" -> respondWithLog(notifications.toast(request))
            "network.request" -> respondWithLog(network.request(request))
            "core.step" -> respondWithLog(core.step(request))
            "runtime.capabilities" -> respondWithLog(BridgeResponse.success(request.id, capabilities(request)).toString())
            "app.log" -> respondWithLog(appLog(request))
            else -> respondWithLog(BridgeResponse.failure(request.id, "unknown_method", "Unknown bridge method: ${request.method}").toString())
        }
    }

    private fun unknownFields(body: JSONObject, allowed: Set<String>): JSONArray {
        val fields = JSONArray()
        val keys = body.keys()
        while (keys.hasNext()) {
            val key = keys.next()
            if (!allowed.contains(key)) {
                fields.put(key)
            }
        }
        return fields
    }

    private fun hasOnlyRuntimeEnvelopeFields(body: JSONObject): Boolean =
        unknownFields(body, runtimeEnvelopeFields).length() == 0

    private fun hasOnlyBridgeRequestFields(body: JSONObject): Boolean =
        unknownFields(body, bridgeRequestFields).length() == 0

    private fun validateBridgeRequest(body: JSONObject?): String? {
        if (body == null) {
            return null
        }
        if (!hasOnlyBridgeRequestFields(body)) {
            return BridgeResponse.failure(
                null,
                "invalid_request",
                "Bridge request contains unknown top-level fields",
                JSONObject(mapOf("fields" to unknownFields(body, bridgeRequestFields))),
            ).toString()
        }
        if (body.opt("id") !is String || body.optString("id").isBlank()) {
            return BridgeResponse.failure(null, "invalid_request", "Bridge request id must be a non-empty string").toString()
        }
        if (body.has("timestamp") && !isFiniteJsonNumber(body.opt("timestamp"))) {
            return BridgeResponse.failure(null, "invalid_request", "Bridge request timestamp must be a finite number").toString()
        }
        if (body.opt("method") !is String) {
            return BridgeResponse.failure(null, "invalid_request", "Bridge request method must be a string").toString()
        }
        if (body.opt("params") !is JSONObject) {
            return BridgeResponse.failure(null, "invalid_request", "Bridge request params must be an object").toString()
        }
        return null
    }

    private fun isFiniteJsonNumber(value: Any?): Boolean =
        value is Number && java.lang.Double.isFinite(value.toDouble())

    private fun appLog(request: BridgeRequest): String {
        val level = request.params.opt("level")
        if (level !is String || level !in setOf("debug", "info", "warn", "error")) {
            return BridgeResponse.failure(
                request.id,
                "invalid_request",
                "app.log level must be debug, info, warn, or error",
            ).toString()
        }
        val message = request.params.opt("message")
        if (message !is String || message.isEmpty()) {
            return BridgeResponse.failure(request.id, "invalid_request", "app.log requires message").toString()
        }
        val limit = request.context.resourceBudget.optInt("maxLogLinesPerMinute", -1)
        if (limit >= 0) {
            val current = bridgeCallCount(request.context.appId, "app.log", seconds = 60)
            if (current >= limit) {
                return BridgeResponse.failure(
                    request.id,
                    "resource_budget_exceeded",
                    "Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute",
                    JSONObject(
                        mapOf(
                            "budget" to "maxLogLinesPerMinute",
                            "current" to current,
                            "max" to limit,
                            "limit" to limit,
                        ),
                    ),
                ).toString()
            }
        }
        val line = "Generated app log [${request.context.appId}] $message"
        when (level) {
            "debug" -> Log.d("TerranePlatformAppLog", line)
            "info" -> Log.i("TerranePlatformAppLog", line)
            "warn" -> Log.w("TerranePlatformAppLog", line)
            "error" -> Log.e("TerranePlatformAppLog", line)
        }
        return BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true))).toString()
    }

    private fun bridgeRateBudgetFailure(request: BridgeRequest): String? {
        val bridgeLimit = request.context.resourceBudget.optInt("maxBridgeCallsPerMinute", -1)
        if (bridgeLimit >= 0) {
            val current = bridgeCallCount(request.context.appId, seconds = 60)
            if (current >= bridgeLimit) {
                return BridgeResponse.failure(
                    request.id,
                    "resource_budget_exceeded",
                    "Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute",
                    JSONObject(
                        mapOf(
                            "appId" to request.context.appId,
                            "budget" to "maxBridgeCallsPerMinute",
                            "current" to current,
                            "max" to bridgeLimit,
                            "limit" to bridgeLimit,
                        ),
                    ),
                ).toString()
            }
        }
        val networkLimit = request.context.resourceBudget.optInt("maxNetworkRequestsPerMinute", -1)
        if (request.method == "network.request" && networkLimit >= 0) {
            val current = bridgeCallCount(request.context.appId, "network.request", seconds = 60)
            if (current >= networkLimit) {
                return BridgeResponse.failure(
                    request.id,
                    "resource_budget_exceeded",
                    "Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute",
                    JSONObject(
                        mapOf(
                            "appId" to request.context.appId,
                            "budget" to "maxNetworkRequestsPerMinute",
                            "current" to current,
                            "max" to networkLimit,
                            "limit" to networkLimit,
                        ),
                    ),
                ).toString()
            }
        }
        return null
    }

    private fun faultInjectionFailure(request: BridgeRequest): String? {
        val sessionId = runtimeSessionId(request)
        var injectedFault: InjectedFault? = null
        database.readableDatabase.rawQuery(
            "SELECT fault_id, code, message, COALESCE(details_json, '{}'), once FROM fault_injections " +
                "WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) " +
                "ORDER BY created_at LIMIT 1",
            arrayOf(request.method, request.context.appId, sessionId),
        ).use { cursor ->
            if (!cursor.moveToFirst()) return@use
            val faultId = cursor.getString(0)
            val details = parseJsonObject(cursor.getString(3)) ?: JSONObject()
            details.put("faultId", faultId)
            details.put("appId", request.context.appId)
            details.put("method", request.method)
            injectedFault = InjectedFault(
                faultId = faultId,
                code = cursor.getString(1),
                message = cursor.getString(2),
                details = details,
                once = cursor.getInt(4) != 0,
            )
        }
        val fault = injectedFault ?: return null
        if (fault.once) {
            disableFaultInjection(fault.faultId)
        }
        return BridgeResponse.failure(request.id, fault.code, fault.message, fault.details).toString()
    }

    private fun disableFaultInjection(faultId: String) {
        database.writableDatabase.update(
            "fault_injections",
            ContentValues().apply { put("enabled", 0) },
            "fault_id = ?",
            arrayOf(faultId),
        )
    }

    private fun bridgeCallCount(appId: String, seconds: Int): Int {
        database.readableDatabase.rawQuery(
            "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND datetime(created_at) >= datetime('now', ?)",
            arrayOf(appId, "-$seconds seconds"),
        ).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getInt(0) else 0
        }
    }

    private fun bridgeCallCount(appId: String, method: String, seconds: Int): Int {
        database.readableDatabase.rawQuery(
            "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND datetime(created_at) >= datetime('now', ?)",
            arrayOf(appId, method, "-$seconds seconds"),
        ).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getInt(0) else 0
        }
    }

    private fun recordBridgeCall(request: BridgeRequest, responseText: String, startedAtMs: Long) {
        if (request.context.appId.isBlank()) return
        val sessionId = ensureRuntimeSession(request)
        val response = parseJsonObject(responseText)
        val values = ContentValues().apply {
            put("bridge_call_id", "bridge_android_${UUID.randomUUID().toString().lowercase()}")
            put("session_id", sessionId)
            put("app_id", request.context.appId)
            putNull("install_id")
            put("method", request.method)
            put("params_json", jsonString(request.params))
            put("result_json", jsonStringOrNull(response?.opt("result")))
            put("error_json", jsonStringOrNull(response?.opt("error")))
            put("duration_ms", SystemClock.elapsedRealtime() - startedAtMs)
            put("created_at", Instant.now().toString())
        }
        database.writableDatabase.insert("bridge_calls", null, values)
    }

    private fun recordCoreStep(request: BridgeRequest, responseText: String) {
        if (request.method != "core.step") return
        val response = parseJsonObject(responseText) ?: return
        if (!response.optBoolean("ok")) return
        val event = request.params.opt("event") ?: return
        val result = response.optJSONObject("result") ?: return
        val sessionId = ensureRuntimeSession(request)
        val eventId = "core_event_android_${UUID.randomUUID().toString().lowercase()}"
        val eventValues = ContentValues().apply {
            put("event_id", eventId)
            put("session_id", sessionId)
            put("app_id", request.context.appId)
            putNull("install_id")
            if (result.has("stateVersion")) {
                put("state_version_before", maxOf(0, result.optInt("stateVersion") - 1))
            } else {
                putNull("state_version_before")
            }
            put("event_json", jsonString(event))
            put("created_at", Instant.now().toString())
        }
        database.writableDatabase.insert("core_events", null, eventValues)
        val actions = result.optJSONArray("actions") ?: JSONArray()
        for (index in 0 until actions.length()) {
            val action = actions.opt(index) ?: continue
            val actionValues = ContentValues().apply {
                put("action_id", "core_action_android_${UUID.randomUUID().toString().lowercase()}")
                put("event_id", eventId)
                put("session_id", sessionId)
                put("app_id", request.context.appId)
                put("action_json", jsonString(action))
                put("created_at", Instant.now().toString())
            }
            database.writableDatabase.insert("core_actions", null, actionValues)
        }
    }

    private fun ensureRuntimeSession(request: BridgeRequest): String {
        val sessionId = runtimeSessionId(request)
        val now = Instant.now().toString()
        val values = ContentValues().apply {
            put("session_id", sessionId)
            put("target", "android")
            put("platform", "android")
            put("runtime_version", "0.1.0")
            put("active_app_id", request.context.appId)
            putNull("active_install_id")
            put("started_at", now)
            put("status", "running")
            put("capabilities_json", "{}")
            put("metadata_json", JSONObject(mapOf("source" to "native-android-bridge")).toString())
        }
        database.writableDatabase.insertWithOnConflict("runtime_sessions", null, values, SQLiteDatabase.CONFLICT_IGNORE)
        database.writableDatabase.update(
            "runtime_sessions",
            ContentValues().apply {
                put("active_app_id", request.context.appId)
                put("status", "running")
            },
            "session_id = ?",
            arrayOf(sessionId),
        )
        return sessionId
    }

    private fun runtimeSessionId(request: BridgeRequest): String =
        "runtime_android_${request.context.appId}_${request.context.mountToken ?: "native"}"

    private data class InjectedFault(
        val faultId: String,
        val code: String,
        val message: String,
        val details: JSONObject,
        val once: Boolean,
    )

    private fun parseJsonObject(text: String): JSONObject? = try {
        JSONObject(text)
    } catch (_: Exception) {
        null
    }

    private fun jsonStringOrNull(value: Any?): String? =
        if (value == null || value == JSONObject.NULL) null else jsonString(value)

    private fun jsonString(value: Any?): String = when (value) {
        null -> "null"
        JSONObject.NULL -> "null"
        is JSONObject -> value.toString()
        is JSONArray -> value.toString()
        is String -> JSONObject.quote(value)
        is Number -> value.toString()
        is Boolean -> value.toString()
        else -> JSONObject.quote(value.toString())
    }

    private fun permissionForBridgeMethod(method: String): String? = when (method) {
        "storage.get", "storage.list" -> "storage.read"
        "storage.set", "storage.remove" -> "storage.write"
        "dialog.openFile", "dialog.saveFile", "notification.toast", "network.request", "core.step" -> method
        else -> null
    }

    private fun capabilities(request: BridgeRequest): JSONObject = JSONObject(
        mapOf(
            "platform" to "android",
            "target" to "android",
            "appId" to request.context.appId,
            "runtimeVersion" to "0.1.0",
            "devMode" to BuildConfig.DEBUG,
            "features" to JSONObject(
                mapOf(
                    "storage.read" to true,
                    "storage.write" to true,
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
            "limits" to JSONObject(
                mapOf("maxPackageBytes" to 1_048_576, "maxFileBytes" to 524_288) +
                    request.context.resourceBudget.toMap(),
            ),
        ),
    )
}

data class AppSandboxContext(
    val appId: String,
    val storagePrefix: String,
    val approvedPermissions: Set<String>,
    val networkPolicy: List<NetworkPolicyRule> = emptyList(),
    val denyPrivateNetwork: Boolean = true,
    val resourceBudget: JSONObject = JSONObject(),
    val mountToken: String? = null,
)

private fun JSONObject.toMap(): Map<String, Any> =
    keys().asSequence().associateWith { key -> opt(key) }

class BridgeRequest(body: JSONObject, val context: AppSandboxContext) {
    val id: String? = body.getString("id")
    val method: String = body.getString("method")
    val params: JSONObject = body.getJSONObject("params")
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
