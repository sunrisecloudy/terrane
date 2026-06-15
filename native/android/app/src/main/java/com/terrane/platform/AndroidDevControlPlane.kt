package com.terrane.platform

import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.database.sqlite.SQLiteDatabase
import android.util.Base64
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.security.SecureRandom
import java.time.Instant
import java.util.Locale
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

class AndroidDevControlPlane(
    private val context: Context,
    private val bridge: NativeBridge,
    requestedPort: Int,
) {
    private val running = AtomicBoolean(false)
    private val database = PlatformDatabase(context)
    private val controlSessionId = "control_android_${UUID.randomUUID().toString().lowercase()}"
    private val token = generateToken()
    private val tokenPath = File(context.filesDir, "control.token")
    private val server = ServerSocket(requestedPort, 50, InetAddress.getByName("127.0.0.1"))
    private var acceptThread: Thread? = null

    val port: Int
        get() = server.localPort

    fun start() {
        if (!BuildConfig.DEBUG) {
            Log.w(tag, "Android dev control plane is disabled in release builds")
            return
        }
        if (!running.compareAndSet(false, true)) return
        writeControlTokenFile()
        insertControlSession()
        acceptThread = thread(name = "TerraneAndroidDevControl", isDaemon = true) {
            while (running.get()) {
                try {
                    server.accept().use { socket -> handleClient(socket) }
                } catch (error: Exception) {
                    if (running.get()) {
                        Log.w(tag, "Android dev control request failed: ${error.message}")
                    }
                }
            }
        }
        Log.i(tag, "TERRANE_ANDROID_CONTROL_READY port=$port tokenPath=${tokenPath.absolutePath}")
    }

    fun stop() {
        if (!running.compareAndSet(true, false)) return
        finishControlSession()
        try {
            server.close()
        } catch (_: Exception) {
        }
    }

    private fun handleClient(socket: Socket) {
        socket.soTimeout = 10_000
        val input = BufferedInputStream(socket.getInputStream())
        val requestLine = readHttpLine(input) ?: return
        val parts = requestLine.split(" ")
        if (parts.size < 2) {
            writeJson(socket, 400, controlError("invalid_request", "Malformed HTTP request"))
            return
        }
        val method = parts[0].uppercase()
        val path = parts[1].substringBefore("?")
        val headers = mutableMapOf<String, String>()
        while (true) {
            val line = readHttpLine(input) ?: return
            if (line.isEmpty()) break
            val separator = line.indexOf(':')
            if (separator > 0) {
                headers[line.substring(0, separator).trim().lowercase()] = line.substring(separator + 1).trim()
            }
        }
        val body = readBody(input, headers["content-length"]?.toIntOrNull() ?: 0)
        val startMs = System.currentTimeMillis()
        val providedToken = headers["x-platform-control-token"]
        if (providedToken != token) {
            audit(path, controlToolForPath(path), method, "rejected", "control_auth_required", null, null, System.currentTimeMillis() - startMs)
            writeJson(socket, 401, controlError("control_auth_required", "A valid X-Platform-Control-Token header is required"))
            return
        }

        val response = route(method, path, body, startMs)
        writeJson(socket, response.status, response.body)
    }

    private fun route(method: String, path: String, body: String, startMs: Long): HttpJsonResponse {
        if (path == "/health" && method == "GET") {
            val result = healthJson()
            audit(path, "platform.health", method, "accepted", null, null, result, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(200, controlOk(result))
        }
        if (path == "/control/sessions" && method == "POST") {
            insertControlSession()
            val result = JSONObject(
                mapOf(
                    "controlSessionId" to controlSessionId,
                    "target" to "android-emulator",
                    "port" to port,
                    "tokenPath" to tokenPath.absolutePath,
                ),
            )
            audit(path, "control.sessions.create", method, "accepted", null, jsonBodyOrNull(body), result, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(200, controlOk(result))
        }

        val sessionRoute = parseSessionRoute(path)
        if (sessionRoute != null && sessionRoute.sessionId == controlSessionId) {
            return routeSessionCommand(method, path, sessionRoute.action, body, startMs)
        }

        if (path == "/control/command" && method == "POST") {
            return handleControlCommand(path, method, body, startMs)
        }

        audit(path, controlToolForPath(path), method, "rejected", "not_found", jsonBodyOrNull(body), null, System.currentTimeMillis() - startMs)
        return HttpJsonResponse(404, controlError("not_found", "Control route not found"))
    }

    private fun routeSessionCommand(method: String, path: String, action: String, body: String, startMs: Long): HttpJsonResponse =
        when {
            action == "snapshot" && method == "GET" -> {
                val result = dbSnapshotJson().put("controlSessionId", controlSessionId)
                audit(path, "control.sessions.snapshot", method, "accepted", null, null, result, System.currentTimeMillis() - startMs)
                HttpJsonResponse(200, controlOk(result))
            }
            action == "events" && method == "GET" -> {
                val result = JSONObject(
                    mapOf(
                        "controlSessionId" to controlSessionId,
                        "controlCommands" to tableRows("control_commands", "control_session_id", controlSessionId),
                        "bridgeCalls" to tableRows("bridge_calls"),
                        "coreEvents" to tableRows("core_events"),
                    ),
                )
                audit(path, "control.sessions.events", method, "accepted", null, null, result, System.currentTimeMillis() - startMs)
                HttpJsonResponse(200, controlOk(result))
            }
            action == "capabilities" && method == "GET" -> {
                val result = controlCapabilitiesJson()
                audit(path, "control.sessions.capabilities", method, "accepted", null, null, result, System.currentTimeMillis() - startMs)
                HttpJsonResponse(200, controlOk(result))
            }
            action == "command" && method == "POST" -> handleControlCommand(path, method, body, startMs)
            action == "end" && (method == "POST" || method == "DELETE") -> {
                finishControlSession()
                val result = JSONObject(mapOf("controlSessionId" to controlSessionId, "status" to "ended"))
                audit(path, "control.sessions.end", method, "accepted", null, jsonBodyOrNull(body), result, System.currentTimeMillis() - startMs)
                HttpJsonResponse(200, controlOk(result))
            }
            else -> {
                audit(path, "control.sessions.$action", method, "rejected", "not_found", jsonBodyOrNull(body), null, System.currentTimeMillis() - startMs)
                HttpJsonResponse(404, controlError("not_found", "Control session route not found"))
            }
        }

    private fun handleControlCommand(path: String, method: String, body: String, startMs: Long): HttpJsonResponse {
        val parsed = try {
            JSONObject(if (body.isBlank()) "{}" else body)
        } catch (_: Exception) {
            audit(path, "control.command", method, "rejected", "invalid_request", null, null, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(400, controlError("invalid_request", "Control command body must be JSON"))
        }
        val tool = parsed.optString("tool").ifBlank {
            audit(path, "control.command", method, "rejected", "invalid_request", jsonBodyOrNull(body), null, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(400, controlError("invalid_request", "Control command requires tool"))
        }
        val args = parsed.optJSONObject("args") ?: JSONObject()
        val argsForAudit = JSONObject(mapOf("tool" to tool, "args" to args))
        val result = try {
            controlCommandResult(tool, args)
        } catch (error: ControlCommandException) {
            audit(path, tool, method, "rejected", error.code, argsForAudit, null, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(error.status, controlError(error.code, error.message ?: "Control command failed"))
        } catch (error: Exception) {
            audit(path, tool, method, "rejected", "control_command_failed", argsForAudit, null, System.currentTimeMillis() - startMs)
            return HttpJsonResponse(500, controlError("control_command_failed", error.message ?: "Control command failed"))
        }
        audit(path, tool, method, "accepted", null, argsForAudit, result, System.currentTimeMillis() - startMs)
        return HttpJsonResponse(200, controlOk(result))
    }

    private fun controlCommandResult(tool: String, args: JSONObject): JSONObject = when (tool) {
        "platform.health" -> healthJson()
        "platform.list_targets" -> platformListTargetsJson()
        "platform.list_webapps" -> platformListWebappsJson(args)
        "runtime.capabilities" -> bridgeCommand(
            appId = args.optString("appId").ifBlank { "notes-lite" },
            method = "runtime.capabilities",
            params = JSONObject(),
            id = "android_control_capabilities",
        )
        "runtime.call_bridge" -> {
            val appId = requiredString(args, "appId")
            val bridgeMethod = requiredString(args, "method")
            val params = args.optJSONObject("params") ?: JSONObject()
            bridgeCommand(appId, bridgeMethod, params, "android_control_call_bridge")
        }
        "runtime.core_step" -> {
            val appId = args.optString("appId").ifBlank { "task-workbench" }
            val event = args.optJSONObject("event") ?: JSONObject(
                mapOf("type" to "CreateTask", "payload" to JSONObject(mapOf("title" to "Android dev control task"))),
            )
            bridgeCommand(appId, "core.step", JSONObject(mapOf("event" to event)), "android_control_core_step")
        }
        "runtime.accessibility_snapshot" -> runtimeAccessibilitySnapshotJson(args)
        "runtime.run_accessibility_audit" -> runtimeAccessibilityAuditJson(args)
        "runtime.assert_accessibility" -> runtimeAssertAccessibilityJson(args)
        "runtime.core_snapshot" -> runtimeCoreSnapshotJson(args)
        "runtime.replay_events" -> runtimeReplayEventsJson(args)
        "runtime.assert_core_action" -> runtimeAssertCoreActionJson(args)
        "runtime.storage_get" -> {
            val appId = requiredString(args, "appId")
            bridgeCommand(appId, "storage.get", storageGetParams(args), "android_control_storage_get")
        }
        "runtime.storage_set" -> {
            val appId = requiredString(args, "appId")
            bridgeCommand(appId, "storage.set", storageSetParams(args), "android_control_storage_set")
        }
        "runtime.assert_storage" -> runtimeAssertStorage(args)
        "runtime.resource_usage" -> runtimeResourceUsageJson(requiredString(args, "appId"))
        "runtime.event_log" -> runtimeEventLogJson(requiredString(args, "appId"))
        "runtime.console_logs" -> runtimeConsoleLogsJson(requiredString(args, "appId"))
        "runtime.bridge_calls" -> runtimeBridgeCallsJson(optionalString(args, "appId"))
        "runtime.clear_logs" -> runtimeClearLogsJson(args)
        "runtime.notification_capture" -> runtimeNotificationCaptureJson(optionalString(args, "appId"))
        "runtime.assert_bridge_call" -> runtimeAssertBridgeCallJson(args)
        "runtime.assert_no_console_errors" -> runtimeAssertNoConsoleErrorsJson(optionalString(args, "appId"))
        "runtime.storage_reset" -> runtimeStorageResetJson(args, clearRuntimeLogs = false)
        "platform.reset_webapp" -> runtimeStorageResetJson(args, clearRuntimeLogs = true)
        "runtime.fault_inject" -> runtimeFaultInjectJson(args)
        "runtime.network_mock_set" -> runtimeNetworkMockSetJson(args)
        "runtime.network_mock_reset" -> runtimeNetworkMockResetJson(args)
        "runtime.dialog_mock_set" -> runtimeDialogMockSetJson(args)
        "platform.create_snapshot" -> platformCreateSnapshotJson(args)
        "platform.restore_snapshot" -> platformRestoreSnapshotJson(args)
        "runtime.compare_snapshot" -> runtimeCompareSnapshotJson(args)
        "db.snapshot" -> dbSnapshotJson()
        "db.export_backup" -> dbExportBackupJson()
        "db.import_backup" -> dbImportBackupJson(args)
        "db.export_debug_bundle" -> dbExportDebugBundleJson()
        "db.query_app_storage" -> queryRowsJson("app_storage", args, "app_id")
        "db.query_app_versions" -> queryRowsJson("app_versions", args, "app_id")
        "db.query_bridge_calls" -> queryRowsJson("bridge_calls", args, "app_id")
        "db.query_core_events" -> queryRowsJson("core_events", args, "app_id")
        "db.query_test_runs" -> queryRowsJson("test_runs", args, "app_id")
        else -> throw ControlCommandException(404, "unsupported_tool", "Unsupported Android dev control command")
    }

    private fun bridgeCommand(appId: String, method: String, params: JSONObject, id: String): JSONObject =
        bridge.handleControlBridgeCall(appId = appId, method = method, params = params, id = id)

    private fun runtimeAccessibilitySnapshotJson(args: JSONObject): JSONObject {
        val appId = optionalString(args, "appId") ?: "notes-lite"
        val html = htmlForStaticApp(appId)
        return accessibilitySnapshotFromHtml(appId, html)
    }

    private fun runtimeAccessibilityAuditJson(args: JSONObject): JSONObject {
        val appId = optionalString(args, "appId") ?: "notes-lite"
        return accessibilityAuditFromHtml(appId, htmlForStaticApp(appId))
    }

    private fun runtimeAssertAccessibilityJson(args: JSONObject): JSONObject {
        val appId = optionalString(args, "appId") ?: "notes-lite"
        val rule = optionalString(args, "rule")
        val report = accessibilityAuditFromHtml(appId, htmlForStaticApp(appId))
        val checks = report.optJSONArray("checks") ?: JSONArray()
        val failures = JSONArray()
        for (index in 0 until checks.length()) {
            val check = checks.optJSONObject(index) ?: continue
            if (check.optString("status") == "fail" && (rule == null || check.optString("id") == rule)) {
                failures.put(check)
            }
        }
        if (failures.length() > 0) {
            throw ControlCommandException(400, "accessibility_failed", "Accessibility assertion failed")
        }
        return JSONObject()
            .put("ok", true)
            .put("appId", appId)
            .put("rule", rule ?: JSONObject.NULL)
            .put("report", report)
    }

    private fun storageGetParams(args: JSONObject): JSONObject {
        val params = JSONObject().put("key", requiredStorageString(args, "key", "runtime.storage_get requires appId and key"))
        if (args.has("defaultValue")) {
            params.put("defaultValue", args.opt("defaultValue") ?: JSONObject.NULL)
        }
        return params
    }

    private fun storageSetParams(args: JSONObject): JSONObject {
        if (!args.has("value")) {
            throw ControlCommandException(400, "invalid_request", "runtime.storage_set requires appId, key, and value")
        }
        return JSONObject()
            .put("key", requiredStorageString(args, "key", "runtime.storage_set requires appId, key, and value"))
            .put("value", args.opt("value") ?: JSONObject.NULL)
    }

    private fun runtimeAssertStorage(args: JSONObject): JSONObject {
        if (!args.has("value")) {
            throw ControlCommandException(400, "invalid_request", "runtime.assert_storage requires appId, key, and value")
        }
        val appId = requiredString(args, "appId")
        val key = requiredStorageString(args, "key", "runtime.assert_storage requires appId, key, and value")
        val response = bridgeCommand(appId, "storage.get", JSONObject().put("key", key), "android_control_storage_assert_get")
        if (!response.optBoolean("ok")) {
            val error = response.optJSONObject("error")
            throw ControlCommandException(
                400,
                error?.optString("code")?.ifBlank { "assertion_failed" } ?: "assertion_failed",
                error?.optString("message")?.ifBlank { "Storage assertion read failed" } ?: "Storage assertion read failed",
            )
        }
        val actual = response.optJSONObject("result")?.opt("value") ?: JSONObject.NULL
        val expected = args.opt("value") ?: JSONObject.NULL
        if (!jsonValuesEqual(actual, expected)) {
            throw ControlCommandException(400, "assertion_failed", "Storage value did not match expected value")
        }
        return JSONObject()
            .put("ok", true)
            .put("appId", appId)
            .put("key", key)
            .put("value", actual)
    }

    private fun runtimeResourceUsageJson(appId: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("storageKeys", scalarLong("SELECT COUNT(*) FROM app_storage WHERE app_id = ?", arrayOf(appId)))
        .put("storageBytes", scalarLong("SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?", arrayOf(appId)))
        .put("bridgeCalls", scalarLong("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?", arrayOf(appId)))
        .put("coreEvents", scalarLong("SELECT COUNT(*) FROM core_events WHERE app_id = ?", arrayOf(appId)))
        .put("coreActions", scalarLong("SELECT COUNT(*) FROM core_actions WHERE app_id = ?", arrayOf(appId)))
        .put("networkRequestsLastMinute", scalarLong("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'network.request' AND datetime(created_at) >= datetime('now', '-60 seconds')", arrayOf(appId)))
        .put("logLinesLastMinute", scalarLong("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'app.log' AND datetime(created_at) >= datetime('now', '-60 seconds')", arrayOf(appId)))

    private fun runtimeCoreSnapshotJson(args: JSONObject): JSONObject {
        val appId = requiredToolString(args, "appId", "runtime.core_snapshot requires appId")
        return JSONObject()
            .put("appId", appId)
            .put(
                "stateVersion",
                scalarLong(
                    "SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?",
                    arrayOf(appId),
                ),
            )
            .put("coreEvents", coreEventRowsJson(appId))
            .put("coreActions", coreActionRowsJson(appId))
    }

    private fun runtimeReplayEventsJson(args: JSONObject): JSONObject {
        val appId = requiredToolString(args, "appId", "runtime.replay_events requires appId")
        val events = args.opt("events") as? JSONArray
            ?: throw ControlCommandException(400, "invalid_request", "runtime.replay_events events must be an array")
        val replayCore = ForgeCoreBridge()
        val context = AppSandboxContext(
            appId = appId,
            storagePrefix = "$appId:",
            approvedPermissions = setOf("core.step"),
            mountToken = controlSessionId,
        )
        val replay = JSONArray()
        for (index in 0 until events.length()) {
            val event = events.opt(index) ?: JSONObject.NULL
            val request = BridgeRequest(
                JSONObject()
                    .put("id", "control_replay_$index")
                    .put("method", "core.step")
                    .put("params", JSONObject().put("event", event)),
                context,
            )
            val response = parseJsonObject(replayCore.step(request))
            val result = response?.optJSONObject("result") ?: JSONObject()
                .put("ok", false)
                .put(
                    "error",
                    response?.optJSONObject("error") ?: JSONObject()
                        .put("code", "core_error")
                        .put("message", "Replay event failed")
                        .put("details", JSONObject()),
                )
                .put("actions", JSONArray())
            replay.put(
                JSONObject()
                    .put("index", index)
                    .put("event", event)
                    .put("result", result),
            )
        }
        return JSONObject()
            .put("ok", true)
            .put("appId", appId)
            .put("replay", replay)
    }

    private fun runtimeAssertCoreActionJson(args: JSONObject): JSONObject {
        val appId = requiredToolString(args, "appId", "runtime.assert_core_action requires appId")
        val expectedType = typedStringOrNull(args, "type", "runtime.assert_core_action type must be a string")
            ?: typedStringOrNull(args, "actionType", "runtime.assert_core_action type must be a string")
        val expectedMatch = if (args.has("match")) {
            args.opt("match") as? JSONObject
                ?: throw ControlCommandException(400, "invalid_request", "runtime.assert_core_action match must be an object")
        } else {
            null
        }
        val expectedAction = if (args.has("action")) args.opt("action") ?: JSONObject.NULL else null
        val rows = coreActionRowsJson(appId)
        val actions = JSONArray()
        var latest: JSONObject? = null
        var latestAction: JSONObject? = null
        for (index in 0 until rows.length()) {
            val row = rows.getJSONObject(index)
            val action = row.optJSONObject("action") ?: parseJsonObject(row.optString("action_json")) ?: continue
            if (expectedType != null && action.optString("type") != expectedType) continue
            if (expectedAction != null && !jsonValuesEqual(action, expectedAction)) continue
            if (expectedMatch != null && !jsonMatchesSubset(action, expectedMatch)) continue
            actions.put(action)
            latest = row
            latestAction = action
        }
        if (actions.length() == 0) {
            throw ControlCommandException(400, "core_action.not_found", "Expected core action was not found")
        }
        return JSONObject()
            .put("ok", true)
            .put("appId", appId)
            .put("type", expectedType ?: JSONObject.NULL)
            .put("count", actions.length())
            .put("actions", actions)
            .put("latest", latest ?: JSONObject.NULL)
            .put("action", latestAction ?: JSONObject.NULL)
    }

    private fun runtimeEventLogJson(appId: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("bridgeCalls", tableRows("bridge_calls", "app_id", appId))
        .put("coreEvents", coreEventRowsJson(appId))
        .put("coreActions", coreActionRowsJson(appId))

    private fun runtimeConsoleLogsJson(appId: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("logs", consoleLogRows(appId))

    private fun runtimeBridgeCallsJson(appId: String?): JSONObject = JSONObject()
        .put("appId", appId ?: JSONObject.NULL)
        .put("bridgeCalls", bridgeCallRows(appId))

    private fun runtimeClearLogsJson(args: JSONObject): JSONObject {
        val appId = optionalString(args, "appId")
        val db = database.writableDatabase
        val bridgeCalls = deleteLogRows(db, "bridge_calls", appId)
        val coreActions = deleteLogRows(db, "core_actions", appId)
        val coreEvents = deleteLogRows(db, "core_events", appId)
        return JSONObject()
            .put("ok", true)
            .put("appId", appId ?: JSONObject.NULL)
            .put("bridgeCallsCleared", bridgeCalls)
            .put("coreActionsCleared", coreActions)
            .put("coreEventsCleared", coreEvents)
    }

    private fun runtimeNotificationCaptureJson(appId: String?): JSONObject = JSONObject()
        .put("appId", appId ?: JSONObject.NULL)
        .put("notifications", notificationRows(appId))

    private fun runtimeAssertBridgeCallJson(args: JSONObject): JSONObject {
        val appId = requiredToolString(args, "appId", "runtime.assert_bridge_call requires appId and method")
        val method = requiredToolString(args, "method", "runtime.assert_bridge_call requires appId and method")
        val rows = bridgeCallRows(appId, method)
        if (rows.length() == 0) {
            throw ControlCommandException(400, "assertion_failed", "Expected bridge call was not recorded")
        }
        return JSONObject()
            .put("ok", true)
            .put("appId", appId)
            .put("method", method)
            .put("count", rows.length())
            .put("latest", rows.getJSONObject(rows.length() - 1))
    }

    private fun runtimeAssertNoConsoleErrorsJson(appId: String?): JSONObject {
        val logs = consoleLogRows(appId)
        var errors = 0
        for (index in 0 until logs.length()) {
            val row = logs.getJSONObject(index)
            val error = row.opt("error")
            if (row.optString("level") == "error" || (error != null && error != JSONObject.NULL)) {
                errors++
            }
        }
        if (errors > 0) {
            throw ControlCommandException(400, "console_errors_found", "Console error logs were found")
        }
        return JSONObject().put("ok", true).put("errors", 0)
    }

    private fun runtimeFaultInjectJson(args: JSONObject): JSONObject {
        val method = faultMethodForArgs(args)
            ?: throw ControlCommandException(400, "invalid_request", "runtime.fault_inject requires a bridge method")
        if (!knownBridgeMethods.contains(method)) {
            throw ControlCommandException(400, "unknown_method", "Unknown bridge method: $method")
        }

        val sessionId = optionalString(args, "sessionId")
        val appId = optionalString(args, "appId")
        if (appId != null && !knownBundledAppIds.contains(appId)) {
            throw ControlCommandException(400, "invalid_request", "runtime.fault_inject appId is not a valid generated app id")
        }
        val code = optionalString(args, "code") ?: "fault_injected"
        val message = optionalString(args, "message") ?: "Injected bridge fault"
        val details = faultDetailsForArgs(args)
        val once = (args.opt("once") as? Boolean) ?: true
        val faultId = "fault_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()

        val values = ContentValues().apply {
            put("fault_id", faultId)
            if (sessionId == null) putNull("session_id") else put("session_id", sessionId)
            if (appId == null) putNull("app_id") else put("app_id", appId)
            put("method", method)
            put("code", code)
            put("message", message)
            put("details_json", jsonString(details))
            put("once", if (once) 1 else 0)
            put("enabled", 1)
            put("created_at", createdAt)
        }
        val inserted = database.writableDatabase.insert("fault_injections", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Fault injection could not be registered")
        }

        return JSONObject()
            .put("ok", true)
            .put("faultId", faultId)
            .put("sessionId", sessionId ?: JSONObject.NULL)
            .put("appId", appId ?: JSONObject.NULL)
            .put("method", method)
            .put("code", code)
            .put("message", message)
            .put("details", details)
            .put("once", once)
    }

    private fun runtimeNetworkMockSetJson(args: JSONObject): JSONObject {
        val urlPattern = networkMockUrlPattern(args)
            ?: throw ControlCommandException(400, "invalid_request", "runtime.network_mock_set requires urlPattern or match.url and response")
        if (!args.has("response") || args.isNull("response")) {
            throw ControlCommandException(400, "invalid_request", "runtime.network_mock_set requires urlPattern or match.url and response")
        }
        val appId = optionalString(args, "appId")
        validateEffectMockAppId(appId)
        val sessionId = optionalString(args, "sessionId")
        val method = networkMockMethod(args)
        val mockId = "netmock_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()
        val values = ContentValues().apply {
            put("mock_id", mockId)
            if (sessionId == null) putNull("session_id") else put("session_id", sessionId)
            if (appId == null) putNull("app_id") else put("app_id", appId)
            put("method", method)
            put("url_pattern", urlPattern)
            put("response_json", jsonString(args.opt("response")))
            put("enabled", 1)
            put("created_at", createdAt)
        }
        val inserted = database.writableDatabase.insert("network_mocks", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Network mock could not be registered")
        }
        return JSONObject()
            .put("ok", true)
            .put("mockId", mockId)
            .put("sessionId", sessionId ?: JSONObject.NULL)
            .put("appId", appId ?: JSONObject.NULL)
            .put("method", method)
            .put("urlPattern", urlPattern)
    }

    private fun runtimeNetworkMockResetJson(args: JSONObject): JSONObject {
        val appId = optionalString(args, "appId")
        validateEffectMockAppId(appId)
        val sessionId = optionalString(args, "sessionId")
        val cleared = when {
            sessionId != null && appId != null -> database.writableDatabase.delete(
                "network_mocks",
                "session_id = ? AND app_id = ?",
                arrayOf(sessionId, appId),
            )
            sessionId != null -> database.writableDatabase.delete("network_mocks", "session_id = ?", arrayOf(sessionId))
            appId != null -> database.writableDatabase.delete("network_mocks", "app_id = ?", arrayOf(appId))
            else -> database.writableDatabase.delete("network_mocks", null, null)
        }
        return JSONObject()
            .put("ok", true)
            .put("cleared", cleared)
    }

    private fun runtimeDialogMockSetJson(args: JSONObject): JSONObject {
        val dialogType = dialogMockType(args)
            ?: throw ControlCommandException(400, "invalid_request", "runtime.dialog_mock_set requires dialogType or method")
        val appId = optionalString(args, "appId")
        validateEffectMockAppId(appId)
        val sessionId = optionalString(args, "sessionId")
        val mockId = "dialogmock_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()
        val values = ContentValues().apply {
            put("mock_id", mockId)
            if (sessionId == null) putNull("session_id") else put("session_id", sessionId)
            if (appId == null) putNull("app_id") else put("app_id", appId)
            put("dialog_type", dialogType)
            put("response_json", jsonString(dialogMockResponse(args)))
            put("enabled", 1)
            put("created_at", createdAt)
        }
        val inserted = database.writableDatabase.insert("dialog_mocks", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Dialog mock could not be registered")
        }
        return JSONObject()
            .put("ok", true)
            .put("mockId", mockId)
            .put("sessionId", sessionId ?: JSONObject.NULL)
            .put("appId", appId ?: JSONObject.NULL)
            .put("dialogType", dialogType)
    }

    private fun validateEffectMockAppId(appId: String?) {
        if (appId != null && !knownBundledAppIds.contains(appId)) {
            throw ControlCommandException(400, "invalid_request", "Runtime effect mock appId is not a valid generated app id")
        }
    }

    private fun networkMockUrlPattern(args: JSONObject): String? {
        val direct = optionalString(args, "urlPattern")
        if (direct != null) return direct
        val match = args.optJSONObject("match") ?: return null
        return optionalString(match, "urlPattern") ?: optionalString(match, "url")
    }

    private fun networkMockMethod(args: JSONObject): String {
        val match = args.optJSONObject("match")
        return (optionalString(args, "method") ?: match?.let { optionalString(it, "method") } ?: "GET")
            .uppercase(Locale.US)
    }

    private fun dialogMockType(args: JSONObject): String? {
        val raw = optionalString(args, "dialogType") ?: optionalString(args, "method") ?: return null
        val normalized = raw.removePrefix("dialog.")
        return if (normalized == "openFile" || normalized == "saveFile") normalized else null
    }

    private fun dialogMockResponse(args: JSONObject): Any {
        if (args.has("response") && !args.isNull("response")) {
            return args.opt("response") ?: JSONObject()
        }
        val cancelled = (args.opt("cancelled") as? Boolean) ?: false
        return JSONObject()
            .put("files", args.opt("files") ?: JSONArray())
            .put("selectedPath", args.opt("selectedPath") ?: JSONObject.NULL)
            .put("cancelled", cancelled)
    }

    private fun faultMethodForArgs(args: JSONObject): String? {
        val method = optionalString(args, "method")
        if (method != null) return method
        return when (val kind = optionalString(args, "kind")) {
            "storage.read" -> "storage.get"
            "storage.write" -> "storage.set"
            "network", "network.request" -> "network.request"
            "core", "core.step" -> "core.step"
            else -> kind
        }
    }

    private fun faultDetailsForArgs(args: JSONObject): Any {
        if (args.has("details") && !args.isNull("details")) {
            return args.opt("details") ?: JSONObject()
        }
        val kind = optionalString(args, "kind")
        return if (kind == null) JSONObject() else JSONObject().put("kind", kind)
    }

    private fun platformListTargetsJson(): JSONObject =
        JSONObject()
            .put(
                "targets",
                JSONArray()
                    .put(
                        JSONObject()
                            .put("id", "android-emulator")
                            .put("platform", "android")
                            .put("status", "available")
                            .put("runtimeVersion", "0.1.0")
                            .put(
                                "controlPlane",
                                JSONObject()
                                    .put("port", port)
                                    .put("debug", true),
                            ),
                    ),
            )

    private fun platformListWebappsJson(args: JSONObject): JSONObject {
        val includeUninstalled = args.optBoolean("includeUninstalled", false)
        val apps = JSONArray()
        val installedIds = mutableSetOf<String>()

        database.readableDatabase.rawQuery(
            "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version, " +
                "a.created_at, a.updated_at, v.runtime_version, v.trust_level " +
                "FROM apps a LEFT JOIN app_versions v ON v.install_id = a.active_install_id " +
                "WHERE (? = 1 OR a.status <> 'uninstalled') ORDER BY a.id",
            arrayOf(if (includeUninstalled) "1" else "0"),
        ).use { cursor ->
            while (cursor.moveToNext()) {
                val appId = cursor.getString(0)
                installedIds.add(appId)
                apps.put(
                    JSONObject()
                        .put("appId", appId)
                        .put("name", cursor.nullableString(1))
                        .put("status", cursor.nullableString(2))
                        .put("activeInstallId", cursor.nullableString(3))
                        .put("activeVersion", cursor.nullableString(4))
                        .put("dataVersion", cursor.getLong(5))
                        .put("runtimeVersion", cursor.nullableString(8))
                        .put("trustLevel", cursor.nullableString(9))
                        .put("createdAt", cursor.nullableString(6))
                        .put("updatedAt", cursor.nullableString(7))
                        .put("bundled", false)
                        .put("installed", true),
                )
            }
        }

        for (appId in knownBundledAppIds) {
            appendBundledWebapp(apps, appId, installedIds)
        }
        return JSONObject().put("apps", apps)
    }

    private fun appendBundledWebapp(apps: JSONArray, appId: String, installedIds: Set<String>) {
        if (installedIds.contains(appId)) return
        val manifest = bundledManifest(appId)
        apps.put(
            JSONObject()
                .put("appId", manifest?.optString("id")?.ifBlank { appId } ?: appId)
                .put("name", manifest?.optString("name")?.ifBlank { appId } ?: appId)
                .put("version", manifest?.optString("version")?.ifBlank { JSONObject.NULL } ?: JSONObject.NULL)
                .put("description", manifest?.optString("description")?.ifBlank { JSONObject.NULL } ?: JSONObject.NULL)
                .put("status", "bundled")
                .put("dataVersion", manifest?.optLong("dataVersion", 1L) ?: 1L)
                .put("bundled", true)
                .put("installed", false),
        )
    }

    private fun bundledManifest(appId: String): JSONObject? =
        try {
            context.assets.open("webapps/examples/$appId/manifest.json").bufferedReader(Charsets.UTF_8).use { reader ->
                JSONObject(reader.readText())
            }
        } catch (_: Exception) {
            null
        }

    private fun htmlForStaticApp(appId: String): String {
        installedPackageFile(appId, "index.html")?.let { return it }
        return try {
            context.assets.open("webapps/examples/$appId/index.html").bufferedReader(Charsets.UTF_8).use { reader ->
                reader.readText()
            }
        } catch (_: Exception) {
            throw ControlCommandException(404, "app_not_found", "Generated app HTML was not found")
        }
    }

    private fun installedPackageFile(appId: String, path: String): String? {
        database.readableDatabase.rawQuery(
            "SELECT f.content_text FROM apps a JOIN app_files f ON f.install_id = a.active_install_id WHERE a.id = ? AND f.path = ? LIMIT 1",
            arrayOf(appId, path),
        ).use { cursor ->
            return if (cursor.moveToFirst()) cursor.nullableStringValue(0) else null
        }
    }

    private fun accessibilitySnapshotFromHtml(appId: String, html: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("title", firstHtmlMatch(html, "<title[^>]*>([\\s\\S]*?)</title>"))
        .put("landmarks", landmarkRecords(html))
        .put("headings", headingRecords(html))
        .put("controls", controlRecords(html))

    private fun accessibilityAuditFromHtml(appId: String, html: String): JSONObject {
        val snapshot = accessibilitySnapshotFromHtml(appId, html)
        val title = snapshot.optString("title")
        val landmarks = snapshot.optJSONArray("landmarks") ?: JSONArray()
        val headings = snapshot.optJSONArray("headings") ?: JSONArray()
        val controls = snapshot.optJSONArray("controls") ?: JSONArray()
        val unlabeled = firstUnlabeledControl(controls)
        val checks = JSONArray()
            .put(accessibilityCheck("document_title", title.isNotBlank(), "Document must include a non-empty <title>."))
            .put(accessibilityCheck("main_landmark", containsRole(landmarks, "main"), "Page must include a <main> landmark."))
            .put(accessibilityCheck("screen_title", containsHeadingLevel(headings, 1), "Page must include an h1 screen title."))
            .put(accessibilityCheck(
                "no_unlabeled_controls",
                unlabeled == null,
                "Every interactive control must have an accessible name.",
                unlabeled?.optString("selector")?.ifBlank { null },
            ))
        return JSONObject()
            .put("appId", appId)
            .put("checkedAt", Instant.now().toString())
            .put("status", if (hasFailingCheck(checks)) "fail" else "pass")
            .put("checks", checks)
    }

    private fun landmarkRecords(html: String): JSONArray {
        val records = JSONArray()
        if (Regex("<main\\b", RegexOption.IGNORE_CASE).containsMatchIn(html)) {
            records.put(JSONObject().put("role", "main").put("selector", "main"))
        }
        return records
    }

    private fun headingRecords(html: String): JSONArray {
        val records = JSONArray()
        for (match in Regex("<h([1-6])\\b[^>]*>([\\s\\S]*?)</h\\1>", RegexOption.IGNORE_CASE).findAll(html)) {
            records.put(
                JSONObject()
                    .put("level", match.groupValues[1].toInt())
                    .put("name", htmlText(match.groupValues[2])),
            )
        }
        return records
    }

    private fun controlRecords(html: String): JSONArray {
        val records = mutableListOf<JSONObject>()
        val paired = Regex("<(button|select|textarea|a)\\b([^>]*)>([\\s\\S]*?)</\\1>", RegexOption.IGNORE_CASE)
        for (match in paired.findAll(html)) {
            val tag = match.groupValues[1].lowercase(Locale.US)
            val attrs = parseHtmlAttrs(match.groupValues[2])
            records.add(controlRecord(html, tag, attrs, match.groupValues[3]))
        }
        val inputs = Regex("<input\\b([^>]*)>", RegexOption.IGNORE_CASE)
        for (match in inputs.findAll(html)) {
            val attrs = parseHtmlAttrs(match.groupValues[1])
            if ((attrs["type"] ?: "text").lowercase(Locale.US) == "hidden") continue
            records.add(controlRecord(html, "input", attrs, ""))
        }
        val sorted = records.sortedBy { it.optString("selector") }
        val array = JSONArray()
        sorted.forEach { array.put(it) }
        return array
    }

    private fun controlRecord(html: String, tag: String, attrs: Map<String, String>, innerHtml: String): JSONObject {
        val testId = attrs["data-testid"].orEmpty()
        val id = attrs["id"].orEmpty()
        val selector = when {
            testId.isNotBlank() -> "[data-testid=\"$testId\"]"
            id.isNotBlank() -> "#$id"
            else -> tag
        }
        return JSONObject()
            .put("tag", tag)
            .put("type", attrs["type"] ?: JSONObject.NULL)
            .put("testId", testId)
            .put("selector", selector)
            .put("name", accessibleName(html, tag, attrs, innerHtml))
    }

    private fun accessibleName(html: String, tag: String, attrs: Map<String, String>, innerHtml: String): String {
        for (attr in listOf("aria-label", "title")) {
            val value = attrs[attr]?.trim()
            if (!value.isNullOrBlank()) return value
        }
        if ((tag == "button" || tag == "a") && htmlText(innerHtml).isNotBlank()) {
            return htmlText(innerHtml)
        }
        val id = attrs["id"]
        if (!id.isNullOrBlank()) {
            labelForId(html, id)?.let { return it }
            wrappingLabelForControl(html, tag, id)?.let { return it }
        }
        return ""
    }

    private fun parseHtmlAttrs(attrsText: String): Map<String, String> {
        val attrs = mutableMapOf<String, String>()
        val pattern = Regex("""\b([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))""")
        for (match in pattern.findAll(attrsText)) {
            val name = match.groupValues[1].lowercase(Locale.US)
            attrs[name] = match.groupValues.getOrElse(2) { "" }.ifBlank {
                match.groupValues.getOrElse(3) { "" }.ifBlank {
                    match.groupValues.getOrElse(4) { "" }
                }
            }
        }
        return attrs
    }

    private fun firstHtmlMatch(html: String, pattern: String): String =
        Regex(pattern, RegexOption.IGNORE_CASE).find(html)?.groupValues?.getOrNull(1)?.let { htmlText(it) }.orEmpty()

    private fun htmlText(html: String): String = html
        .replace(Regex("<script\\b[\\s\\S]*?</script>", RegexOption.IGNORE_CASE), " ")
        .replace(Regex("<style\\b[\\s\\S]*?</style>", RegexOption.IGNORE_CASE), " ")
        .replace(Regex("<[^>]+>"), " ")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace(Regex("[\\s\\n\\r\\t]+"), " ")
        .trim()

    private fun labelForId(html: String, id: String): String? =
        firstHtmlMatch(html, "<label\\b[^>]*\\bfor=[\"']${Regex.escape(id)}[\"'][^>]*>([\\s\\S]*?)</label>").ifBlank { null }

    private fun wrappingLabelForControl(html: String, tag: String, id: String): String? {
        val raw = firstHtmlMatch(html, "<label\\b[^>]*>([\\s\\S]*?<$tag\\b[^>]*\\bid=[\"']${Regex.escape(id)}[\"'][^>]*>[\\s\\S]*?)</label>")
        if (raw.isBlank()) return null
        return raw.replace(Regex("<$tag\\b[\\s\\S]*", RegexOption.IGNORE_CASE), "").let(::htmlText).ifBlank { null }
    }

    private fun accessibilityCheck(id: String, ok: Boolean, message: String, selector: String? = null): JSONObject {
        val check = JSONObject()
            .put("id", id)
            .put("status", if (ok) "pass" else "fail")
            .put("message", message)
        if (!selector.isNullOrBlank()) check.put("selector", selector)
        return check
    }

    private fun firstUnlabeledControl(controls: JSONArray): JSONObject? {
        for (index in 0 until controls.length()) {
            val control = controls.optJSONObject(index) ?: continue
            if (control.optString("name").isBlank()) return control
        }
        return null
    }

    private fun containsRole(records: JSONArray, role: String): Boolean {
        for (index in 0 until records.length()) {
            if (records.optJSONObject(index)?.optString("role") == role) return true
        }
        return false
    }

    private fun containsHeadingLevel(records: JSONArray, level: Int): Boolean {
        for (index in 0 until records.length()) {
            if (records.optJSONObject(index)?.optInt("level") == level) return true
        }
        return false
    }

    private fun hasFailingCheck(checks: JSONArray): Boolean {
        for (index in 0 until checks.length()) {
            if (checks.optJSONObject(index)?.optString("status") == "fail") return true
        }
        return false
    }

    private fun runtimeStorageResetJson(args: JSONObject, clearRuntimeLogs: Boolean): JSONObject {
        if (!args.optBoolean("confirm", false)) {
            throw ControlCommandException(400, "confirmation_required", "Storage reset command requires confirm: true")
        }
        val appId = requiredString(args, "appId")
        val storageRows = tableRows("app_storage", "app_id", appId)
        val snapshotId = "snapshot_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()
        val activeInstallId = activeInstallId(appId)
        val snapshot = JSONObject()
            .put("appId", appId)
            .put("activeInstallId", activeInstallId ?: JSONObject.NULL)
            .put("createdAt", createdAt)
            .put("appStorage", storageRows)
        val snapshotText = snapshot.toString()
        val db = database.writableDatabase
        db.beginTransaction()
        try {
            val values = ContentValues().apply {
                put("snapshot_id", snapshotId)
                putNull("session_id")
                put("app_id", appId)
                if (activeInstallId == null) putNull("install_id") else put("install_id", activeInstallId)
                put("type", "manual")
                put("snapshot_json", snapshotText)
                put("content_hash", "sha256:${sha256Hex(snapshotText)}")
                put("created_at", createdAt)
            }
            db.insertOrThrow("runtime_snapshots", null, values)
            val storageDeleted = db.delete("app_storage", "app_id = ?", arrayOf(appId))
            var bridgeCallsDeleted = 0
            var coreEventsDeleted = 0
            val coreActionsDeleted = if (clearRuntimeLogs) {
                scalarLong("SELECT COUNT(*) FROM core_actions WHERE app_id = ?", arrayOf(appId))
            } else {
                0L
            }
            if (clearRuntimeLogs) {
                bridgeCallsDeleted = db.delete("bridge_calls", "app_id = ?", arrayOf(appId))
                coreEventsDeleted = db.delete("core_events", "app_id = ?", arrayOf(appId))
            }
            db.setTransactionSuccessful()
            return JSONObject()
                .put("ok", true)
                .put("appId", appId)
                .put("snapshotId", snapshotId)
                .put("clearedStorageKeys", storageRows.length())
                .put("storageRowsDeleted", storageDeleted)
                .put("clearedBridgeCalls", bridgeCallsDeleted)
                .put("clearedCoreEvents", coreEventsDeleted)
                .put("clearedCoreActions", coreActionsDeleted)
        } finally {
            db.endTransaction()
        }
    }

    private fun platformCreateSnapshotJson(args: JSONObject): JSONObject {
        val appId = requiredToolString(args, "appId", "platform.create_snapshot requires appId")
        if (!knownBundledAppIds.contains(appId)) {
            throw ControlCommandException(400, "invalid_request", "platform.create_snapshot appId is not a valid generated app id")
        }
        val type = optionalString(args, "type") ?: "manual"
        if (!snapshotTypes.contains(type)) {
            throw ControlCommandException(400, "invalid_request", "Snapshot type is not allowed")
        }
        val sessionId = optionalString(args, "sessionId")
        val metadata = activeAppMetadata(appId)
        val createdAt = Instant.now().toString()
        val storageRows = tableRows("app_storage", "app_id", appId)
        val snapshotId = "snapshot_android_${UUID.randomUUID().toString().lowercase()}"
        val snapshot = JSONObject()
            .put("appId", appId)
            .put("activeInstallId", metadata.activeInstallId ?: JSONObject.NULL)
            .put("activeVersion", metadata.activeVersion ?: JSONObject.NULL)
            .put("dataVersion", metadata.dataVersion)
            .put("storage", storageRows)
            .put("createdAt", createdAt)
        val snapshotText = snapshot.toString()
        val contentHash = "sha256:${sha256Hex(snapshotText)}"
        val values = ContentValues().apply {
            put("snapshot_id", snapshotId)
            if (sessionId == null) putNull("session_id") else put("session_id", sessionId)
            put("app_id", appId)
            if (metadata.activeInstallId == null) putNull("install_id") else put("install_id", metadata.activeInstallId)
            put("type", type)
            put("snapshot_json", snapshotText)
            put("content_hash", contentHash)
            put("created_at", createdAt)
        }
        val inserted = database.writableDatabase.insert("runtime_snapshots", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Snapshot could not be created")
        }
        return JSONObject()
            .put("snapshotId", snapshotId)
            .put("contentHash", contentHash)
            .put("snapshot", snapshot)
            .put("appId", appId)
            .put("activeInstallId", metadata.activeInstallId ?: JSONObject.NULL)
            .put("activeVersion", metadata.activeVersion ?: JSONObject.NULL)
            .put("dataVersion", metadata.dataVersion)
            .put("storage", storageRows)
            .put("createdAt", createdAt)
    }

    private fun platformRestoreSnapshotJson(args: JSONObject): JSONObject {
        if (!args.optBoolean("confirm", false)) {
            throw ControlCommandException(400, "confirmation_required", "platform.restore_snapshot requires confirm: true")
        }
        val snapshotId = requiredToolString(args, "snapshotId", "platform.restore_snapshot requires snapshotId")
        val snapshot = runtimeSnapshotById(snapshotId)
        val appId = optionalString(snapshot, "appId")
        val storage = snapshot.optJSONArray("storage") ?: snapshot.optJSONArray("appStorage") ?: JSONArray()
        val db = database.writableDatabase
        db.beginTransaction()
        try {
            if (appId != null) {
                db.delete("app_storage", "app_id = ?", arrayOf(appId))
            }
            var restored = 0
            val updatedAt = Instant.now().toString()
            for (index in 0 until storage.length()) {
                val row = storage.optJSONObject(index)
                    ?: throw ControlCommandException(400, "invalid_request", "Snapshot storage row must be an object")
                val inserted = db.insertWithOnConflict("app_storage", null, storageSnapshotValues(row, appId, updatedAt), SQLiteDatabase.CONFLICT_REPLACE)
                if (inserted < 0) {
                    throw ControlCommandException(400, "sqlite_error", "Snapshot storage row could not be restored")
                }
                restored++
            }
            if (appId != null && snapshot.has("activeInstallId") && !snapshot.isNull("activeInstallId")) {
                db.update(
                    "apps",
                    ContentValues().apply {
                        put("active_install_id", snapshot.optString("activeInstallId"))
                        if (snapshot.has("activeVersion") && !snapshot.isNull("activeVersion")) {
                            put("active_version", snapshot.optString("activeVersion"))
                        } else {
                            putNull("active_version")
                        }
                        put("data_version", snapshot.optLong("dataVersion", 1L))
                        put("status", "enabled")
                        put("updated_at", updatedAt)
                    },
                    "id = ?",
                    arrayOf(appId),
                )
            }
            db.setTransactionSuccessful()
            return JSONObject()
                .put("ok", true)
                .put("snapshotId", snapshotId)
                .put("appId", appId ?: JSONObject.NULL)
                .put("restoredStorageKeys", restored)
        } finally {
            db.endTransaction()
        }
    }

    private fun runtimeCompareSnapshotJson(args: JSONObject): JSONObject {
        val left = snapshotArgument(args, "left", "leftSnapshotId")
        val right = snapshotArgument(args, "right", "rightSnapshotId")
        val leftComparable = comparableSnapshotJson(left)
        val rightComparable = comparableSnapshotJson(right)
        val leftHash = "sha256:${sha256Hex(leftComparable)}"
        val rightHash = "sha256:${sha256Hex(rightComparable)}"
        val equal = leftComparable == rightComparable
        return JSONObject()
            .put("ok", equal)
            .put("equal", equal)
            .put("leftHash", leftHash)
            .put("rightHash", rightHash)
    }

    private fun runtimeSnapshotById(snapshotId: String): JSONObject {
        database.readableDatabase.rawQuery(
            "SELECT snapshot_json FROM runtime_snapshots WHERE snapshot_id = ?",
            arrayOf(snapshotId),
        ).use { cursor ->
            if (cursor.moveToFirst()) {
                return parseJsonObject(cursor.getString(0))
                    ?: throw ControlCommandException(400, "invalid_request", "Runtime snapshot JSON is invalid")
            }
        }
        throw ControlCommandException(404, "snapshot_not_found", "Runtime snapshot not found: $snapshotId")
    }

    private fun snapshotArgument(args: JSONObject, valueKey: String, snapshotIdKey: String): JSONObject {
        val snapshotId = optionalString(args, snapshotIdKey)
        if (snapshotId != null) {
            return runtimeSnapshotById(snapshotId)
        }
        val value = args.opt(valueKey)
        if (value is JSONObject) {
            return value
        }
        throw ControlCommandException(400, "invalid_request", "runtime.compare_snapshot requires left/right snapshots or snapshot ids")
    }

    private fun storageSnapshotValues(row: JSONObject, fallbackAppId: String?, updatedAt: String): ContentValues {
        val appId = optionalString(row, "app_id") ?: optionalString(row, "appId") ?: fallbackAppId
        val key = optionalString(row, "key")
        if (appId.isNullOrBlank() || key.isNullOrBlank()) {
            throw ControlCommandException(400, "invalid_request", "Snapshot storage row requires app_id and key")
        }
        if (fallbackAppId != null && appId != fallbackAppId) {
            throw ControlCommandException(400, "invalid_request", "Snapshot storage row app_id does not match snapshot appId")
        }
        if (!key.startsWith("$appId:")) {
            throw ControlCommandException(400, "invalid_request", "Snapshot storage key is outside app storage prefix")
        }
        return ContentValues().apply {
            put("app_id", appId)
            put("key", key)
            put("value_json", storageSnapshotValueJson(row))
            put("updated_at", updatedAt)
        }
    }

    private fun storageSnapshotValueJson(row: JSONObject): String {
        val rawValueJson = optionalString(row, "value_json") ?: optionalString(row, "valueJson")
        if (rawValueJson != null) return rawValueJson
        return jsonString(row.opt("value") ?: JSONObject.NULL)
    }

    private fun comparableSnapshotJson(snapshot: JSONObject): String =
        comparableJsonValue(snapshot, storageContext = false)

    private fun comparableJsonValue(value: Any?, storageContext: Boolean): String = when (value) {
        null, JSONObject.NULL -> "null"
        is JSONObject -> comparableJsonObject(value)
        is JSONArray -> {
            val values = (0 until value.length()).map { index -> value.opt(index) ?: JSONObject.NULL }
            val normalized = if (storageContext) values.sortedWith { left, right ->
                storageSortKey(left).compareTo(storageSortKey(right))
            } else values
            normalized.joinToString(prefix = "[", postfix = "]", separator = ",") { item ->
                comparableJsonValue(item, storageContext = false)
            }
        }
        is String -> JSONObject.quote(value)
        is Number -> value.toString()
        is Boolean -> value.toString()
        else -> JSONObject.quote(value.toString())
    }

    private fun comparableJsonObject(value: JSONObject): String {
        val members = mutableMapOf<String, Any?>()
        val keys = value.keys()
        while (keys.hasNext()) {
            val key = keys.next()
            if (snapshotCompareSkipFields.contains(key)) continue
            val normalizedKey = if (key == "appStorage") "storage" else key
            members[normalizedKey] = value.opt(key) ?: JSONObject.NULL
        }
        return members.keys.sorted().joinToString(prefix = "{", postfix = "}", separator = ",") { key ->
            val child = members[key]
            "${JSONObject.quote(key)}:${comparableJsonValue(child, storageContext = key == "storage")}"
        }
    }

    private fun storageSortKey(value: Any?): String {
        val row = value as? JSONObject ?: return ""
        val appId = optionalString(row, "app_id") ?: optionalString(row, "appId") ?: ""
        val key = optionalString(row, "key") ?: ""
        return "$appId|$key"
    }

    private fun consoleLogRows(appId: String?): JSONArray {
        val rows = JSONArray()
        val sql = if (appId == null) {
            "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at LIMIT 100"
        } else {
            "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE app_id = ? AND method = 'app.log' ORDER BY created_at LIMIT 100"
        }
        val selectionArgs = if (appId == null) emptyArray() else arrayOf(appId)
        database.readableDatabase.rawQuery(sql, selectionArgs).use { cursor ->
            while (cursor.moveToNext()) {
                val params = parseJsonObject(cursor.getString(2)) ?: JSONObject()
                val level = params.optString("level")
                val message = params.optString("message")
                rows.put(
                    JSONObject()
                        .put("bridgeCallId", cursor.getString(0))
                        .put("appId", cursor.getString(1))
                        .put("level", if (level.isBlank()) JSONObject.NULL else level)
                        .put("message", if (message.isBlank()) JSONObject.NULL else message)
                        .put("params", params)
                        .put("result", parseJsonValue(cursor.nullableStringValue(3)))
                        .put("error", parseJsonValue(cursor.nullableStringValue(4)))
                        .put("createdAt", cursor.getString(5)),
                )
            }
        }
        return rows
    }

    private fun notificationRows(appId: String?): JSONArray {
        val rows = JSONArray()
        val sql = if (appId == null) {
            "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' ORDER BY created_at LIMIT 100"
        } else {
            "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE app_id = ? AND method = 'notification.toast' ORDER BY created_at LIMIT 100"
        }
        val selectionArgs = if (appId == null) emptyArray() else arrayOf(appId)
        database.readableDatabase.rawQuery(sql, selectionArgs).use { cursor ->
            while (cursor.moveToNext()) {
                val params = parseJsonObject(cursor.getString(2)) ?: JSONObject()
                val message = params.optString("message")
                val level = params.optString("level")
                rows.put(
                    JSONObject()
                        .put("bridgeCallId", cursor.getString(0))
                        .put("appId", cursor.getString(1))
                        .put("message", if (message.isBlank()) JSONObject.NULL else message)
                        .put("level", if (level.isBlank()) JSONObject.NULL else level)
                        .put("params", params)
                        .put("result", parseJsonValue(cursor.nullableStringValue(3)))
                        .put("error", parseJsonValue(cursor.nullableStringValue(4)))
                        .put("createdAt", cursor.getString(5)),
                )
            }
        }
        return rows
    }

    private fun bridgeCallRows(appId: String?, method: String? = null): JSONArray {
        val rows = JSONArray()
        val base = "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls"
        val sql = when {
            appId == null && method == null -> "$base ORDER BY created_at"
            appId == null -> "$base WHERE method = ? ORDER BY created_at"
            method == null -> "$base WHERE app_id = ? ORDER BY created_at"
            else -> "$base WHERE app_id = ? AND method = ? ORDER BY created_at"
        }
        val selectionArgs = when {
            appId == null && method == null -> emptyArray()
            appId == null -> arrayOf(method)
            method == null -> arrayOf(appId)
            else -> arrayOf(appId, method)
        }
        database.readableDatabase.rawQuery(sql, selectionArgs).use { cursor ->
            while (cursor.moveToNext()) {
                rows.put(
                    JSONObject()
                        .put("bridgeCallId", cursor.getString(0))
                        .put("sessionId", cursor.nullableStringValue(1) ?: JSONObject.NULL)
                        .put("appId", cursor.nullableStringValue(2) ?: JSONObject.NULL)
                        .put("installId", cursor.nullableStringValue(3) ?: JSONObject.NULL)
                        .put("method", cursor.getString(4))
                        .put("params", parseJsonValue(cursor.nullableStringValue(5)))
                        .put("result", parseJsonValue(cursor.nullableStringValue(6)))
                        .put("error", parseJsonValue(cursor.nullableStringValue(7)))
                        .put("durationMs", if (cursor.isNull(8)) JSONObject.NULL else cursor.getLong(8))
                        .put("createdAt", cursor.getString(9)),
                )
            }
        }
        return rows
    }

    private fun deleteLogRows(db: SQLiteDatabase, table: String, appId: String?): Int =
        if (appId == null) db.delete(table, null, null) else db.delete(table, "app_id = ?", arrayOf(appId))

    private fun scalarLong(sql: String, selectionArgs: Array<String>): Long {
        database.readableDatabase.rawQuery(sql, selectionArgs).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getLong(0) else 0L
        }
    }

    private fun activeInstallId(appId: String): String? {
        database.readableDatabase.rawQuery("SELECT active_install_id FROM apps WHERE id = ?", arrayOf(appId)).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getString(0) else null
        }
    }

    private fun activeAppMetadata(appId: String): ActiveAppMetadata {
        database.readableDatabase.rawQuery("SELECT active_install_id, active_version, data_version FROM apps WHERE id = ?", arrayOf(appId)).use { cursor ->
            if (cursor.moveToFirst()) {
                return ActiveAppMetadata(
                    activeInstallId = cursor.nullableStringValue(0),
                    activeVersion = cursor.nullableStringValue(1),
                    dataVersion = if (cursor.isNull(2)) 1L else cursor.getLong(2),
                )
            }
        }
        val manifest = bundledManifest(appId)
        return ActiveAppMetadata(
            activeInstallId = null,
            activeVersion = manifest?.optString("version")?.ifBlank { null },
            dataVersion = manifest?.optLong("dataVersion", 1L) ?: 1L,
        )
    }

    private fun healthJson(): JSONObject = JSONObject(
        mapOf(
            "name" to "android",
            "target" to "android-emulator",
            "runtimeVersion" to "0.1.0",
            "db" to "sqlite",
            "controlSessionId" to controlSessionId,
            "port" to port,
            "tokenPath" to tokenPath.absolutePath,
            "capabilities" to controlCapabilitiesJson(),
        ),
    )

    private fun controlCapabilitiesJson(): JSONObject = JSONObject(
        mapOf(
            "platform" to "android",
            "target" to "android-emulator",
            "runtimeVersion" to "0.1.0",
            "controlPlane" to true,
            "tools" to JSONArray(
                listOf(
                    "platform.health",
                    "platform.list_targets",
                    "platform.list_webapps",
                    "runtime.capabilities",
                    "runtime.call_bridge",
                    "runtime.core_step",
                    "runtime.accessibility_snapshot",
                    "runtime.run_accessibility_audit",
                    "runtime.assert_accessibility",
                    "runtime.core_snapshot",
                    "runtime.replay_events",
                    "runtime.assert_core_action",
                    "runtime.storage_get",
                    "runtime.storage_set",
                    "runtime.assert_storage",
                    "runtime.resource_usage",
                    "runtime.event_log",
                    "runtime.console_logs",
                    "runtime.bridge_calls",
                    "runtime.clear_logs",
                    "runtime.notification_capture",
                    "runtime.assert_bridge_call",
                    "runtime.assert_no_console_errors",
                    "runtime.storage_reset",
                    "platform.reset_webapp",
                    "runtime.fault_inject",
                    "runtime.network_mock_set",
                    "runtime.network_mock_reset",
                    "runtime.dialog_mock_set",
                    "platform.create_snapshot",
                    "platform.restore_snapshot",
                    "runtime.compare_snapshot",
                    "db.snapshot",
                    "db.export_backup",
                    "db.import_backup",
                    "db.export_debug_bundle",
                    "db.query_app_storage",
                    "db.query_app_versions",
                    "db.query_bridge_calls",
                    "db.query_core_events",
                    "db.query_test_runs",
                ),
            ),
        ),
    )

    private fun dbSnapshotJson(): JSONObject = JSONObject(
        mapOf(
            "apps" to tableRows("apps"),
            "app_versions" to tableRows("app_versions"),
            "app_files" to tableRows("app_files"),
            "app_permissions" to tableRows("app_permissions"),
            "app_installations" to tableRows("app_installations"),
            "app_storage" to tableRows("app_storage"),
            "app_install_reports" to tableRows("app_install_reports"),
            "app_migrations" to tableRows("app_migrations"),
            "migration_runs" to tableRows("migration_runs"),
            "runtime_sessions" to tableRows("runtime_sessions"),
            "bridge_calls" to tableRows("bridge_calls"),
            "core_events" to tableRows("core_events"),
            "core_actions" to tableRows("core_actions"),
            "runtime_snapshots" to tableRows("runtime_snapshots"),
            "control_sessions" to tableRows("control_sessions"),
            "control_commands" to tableRows("control_commands"),
            "test_runs" to tableRows("test_runs"),
            "backup_exports" to tableRows("backup_exports"),
        ),
    )

    private fun dbExportDebugBundleJson(): JSONObject {
        val exportId = "debugbundle_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()
        val document = JSONObject()
            .put("exportId", exportId)
            .put("type", "debug-bundle")
            .put("source", JSONObject()
                .put("platform", "android")
                .put("target", "android-emulator"),
            )
            .put("runtimeVersion", androidRuntimeVersion)
            .put("createdAt", createdAt)
            .put("apps", tableRows("apps"))
            .put("appVersions", tableRows("app_versions"))
            .put("appFiles", tableRows("app_files"))
            .put("appPermissions", tableRows("app_permissions"))
            .put("appStorage", tableRows("app_storage"))
            .put("appInstallReports", tableRows("app_install_reports"))
            .put("runtimeCapabilities", controlCapabilitiesJson())
            .put("debug", JSONObject()
                .put("runtimeSessions", tableRows("runtime_sessions"))
                .put("bridgeCalls", tableRows("bridge_calls"))
                .put("controlSessions", tableRows("control_sessions"))
                .put("controlCommands", tableRows("control_commands"))
                .put("coreEvents", tableRows("core_events"))
                .put("coreActions", tableRows("core_actions"))
                .put("runtimeSnapshots", tableRows("runtime_snapshots"))
                .put("testRuns", tableRows("test_runs")),
            )
        val contentHash = "sha256:${sha256Hex(document.toString())}"
        document.put("contentHash", contentHash)
        val values = ContentValues().apply {
            put("export_id", exportId)
            put("type", "debug-bundle")
            put("source_platform", "android")
            put("runtime_version", androidRuntimeVersion)
            put("export_json", document.toString())
            put("content_hash", contentHash)
            put("created_at", createdAt)
            putNull("imported_at")
        }
        val inserted = database.writableDatabase.insert("backup_exports", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Could not export debug bundle")
        }
        return document
    }

    private fun dbExportBackupJson(): JSONObject {
        val exportId = "backup_android_${UUID.randomUUID().toString().lowercase()}"
        val createdAt = Instant.now().toString()
        val document = JSONObject()
            .put("exportId", exportId)
            .put("type", "backup")
            .put("source", JSONObject()
                .put("platform", "android")
                .put("target", "android-emulator"),
            )
            .put("runtimeVersion", androidRuntimeVersion)
            .put("createdAt", createdAt)
            .put("apps", tableRows("apps"))
            .put("appVersions", tableRows("app_versions"))
            .put("appFiles", tableRows("app_files"))
            .put("appPermissions", tableRows("app_permissions"))
            .put("appStorage", tableRows("app_storage"))
            .put("appMigrations", tableRows("app_migrations"))
            .put("appInstallReports", tableRows("app_install_reports"))
            .put("runtimeCapabilities", controlCapabilitiesJson())
            .put("debug", JSONObject())
        val contentHash = "sha256:${sha256Hex(document.toString())}"
        document.put("contentHash", contentHash)
        val values = ContentValues().apply {
            put("export_id", exportId)
            put("type", "backup")
            put("source_platform", "android")
            put("runtime_version", androidRuntimeVersion)
            put("export_json", document.toString())
            put("content_hash", contentHash)
            put("created_at", createdAt)
            putNull("imported_at")
        }
        val inserted = database.writableDatabase.insert("backup_exports", null, values)
        if (inserted < 0) {
            throw ControlCommandException(400, "sqlite_error", "Could not export backup")
        }
        return document
    }

    private fun dbImportBackupJson(args: JSONObject): JSONObject {
        val document = args.opt("backup") as? JSONObject
            ?: throw ControlCommandException(400, "invalid_request", "db.import_backup requires backup")
        val type = backupString(document, "type")
        if (!setOf("backup", "debug-bundle", "test-fixture").contains(type)) {
            throw ControlCommandException(400, "invalid_request", "Backup import requires type backup, debug-bundle, or test-fixture")
        }
        val apps = requiredBackupArray(document, "apps")
        val appVersions = requiredBackupArray(document, "appVersions")
        val appFiles = requiredBackupArray(document, "appFiles")
        val appPermissions = requiredBackupArray(document, "appPermissions")
        val appStorage = requiredBackupArray(document, "appStorage")
        val appMigrations = document.optJSONArray("appMigrations") ?: JSONArray()
        val appInstallReports = document.optJSONArray("appInstallReports") ?: JSONArray()
        val createdAt = Instant.now().toString()
        val db = database.writableDatabase
        db.beginTransaction()
        try {
            importApps(db, apps, createdAt)
            importAppVersions(db, appVersions, createdAt)
            importAppFiles(db, appFiles, createdAt)
            importAppPermissions(db, appPermissions)
            importAppStorage(db, appStorage, createdAt)
            importAppMigrations(db, appMigrations, createdAt)
            importAppInstallReports(db, appInstallReports, createdAt)
            val source = document.optJSONObject("source")
            val documentText = document.toString()
            val importId = "import_android_${UUID.randomUUID().toString().lowercase()}"
            val inserted = db.insert("backup_exports", null, ContentValues().apply {
                put("export_id", importId)
                put("type", "import")
                put("source_platform", source?.let { backupString(it, "platform") } ?: "unknown")
                put("runtime_version", backupString(document, "runtimeVersion") ?: androidRuntimeVersion)
                put("export_json", documentText)
                put("content_hash", backupString(document, "contentHash") ?: "sha256:${sha256Hex(documentText)}")
                put("created_at", createdAt)
                put("imported_at", createdAt)
            })
            if (inserted < 0) {
                throw ControlCommandException(400, "sqlite_error", "Backup import could not be completed")
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
        return JSONObject()
            .put("ok", true)
            .put("apps", apps.length())
            .put("appVersions", appVersions.length())
            .put("appStorage", appStorage.length())
    }

    private fun importApps(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val app = backupObjectAt(rows, index, "apps")
            val appId = requiredBackupString(app, "id", "appId")
            val inserted = db.insertWithOnConflict("apps", null, ContentValues().apply {
                put("id", appId)
                put("name", backupString(app, "name") ?: appId)
                put("status", backupString(app, "status") ?: "enabled")
                putNullable("active_install_id", backupString(app, "active_install_id", "activeInstallId"))
                putNullable("active_version", backupString(app, "active_version", "activeVersion"))
                put("data_version", backupLong(app, "data_version", "dataVersion", default = 1L))
                put("created_at", backupString(app, "created_at", "createdAt") ?: createdAt)
                put("updated_at", backupString(app, "updated_at", "updatedAt") ?: createdAt)
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppVersions(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val version = backupObjectAt(rows, index, "appVersions")
            val installId = requiredBackupString(version, "install_id", "installId")
            val appId = requiredBackupString(version, "app_id", "appId")
            val appVersion = requiredBackupString(version, "version", "appVersion")
            val inserted = db.insertWithOnConflict("app_versions", null, ContentValues().apply {
                put("install_id", installId)
                put("app_id", appId)
                put("version", appVersion)
                put("runtime_version", backupString(version, "runtime_version", "runtimeVersion") ?: androidRuntimeVersion)
                put("data_version", backupLong(version, "data_version", "dataVersion", default = 1L))
                put("manifest_json", backupJsonText(version, stringKeys = listOf("manifest_json", "manifestJson"), valueKeys = listOf("manifest"), fallback = "{}"))
                put("manifest_hash", backupString(version, "manifest_hash", "manifestHash") ?: "")
                put("content_hash", backupString(version, "content_hash", "contentHash") ?: "")
                putNullable("signature_json", backupJsonText(version, stringKeys = listOf("signature_json", "signatureJson"), valueKeys = listOf("signature"), fallback = null))
                put("trust_level", backupString(version, "trust_level", "trustLevel") ?: "developer")
                put("status", backupString(version, "status") ?: "installed")
                put("created_at", backupString(version, "created_at", "installedAt", "createdAt") ?: createdAt)
                putNullable("activated_at", backupString(version, "activated_at", "activatedAt"))
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppFiles(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val file = backupObjectAt(rows, index, "appFiles")
            val installId = requiredBackupString(file, "install_id", "installId")
            val path = requiredBackupString(file, "path")
            val contentText = backupString(file, "content_text", "contentText") ?: ""
            val inserted = db.insertWithOnConflict("app_files", null, ContentValues().apply {
                put("install_id", installId)
                put("path", path)
                put("content_text", contentText)
                put("content_hash", backupString(file, "content_hash", "contentHash") ?: "sha256:${sha256Hex(contentText)}")
                put("size_bytes", backupLong(file, "size_bytes", "sizeBytes", default = contentText.toByteArray(Charsets.UTF_8).size.toLong()))
                put("mime", backupString(file, "mime") ?: "text/plain")
                put("created_at", backupString(file, "created_at", "createdAt") ?: createdAt)
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppPermissions(db: SQLiteDatabase, rows: JSONArray) {
        for (index in 0 until rows.length()) {
            val permission = backupObjectAt(rows, index, "appPermissions")
            val installId = requiredBackupString(permission, "install_id", "installId")
            val appId = requiredBackupString(permission, "app_id", "appId")
            val permissionName = requiredBackupString(permission, "permission")
            val inserted = db.insertWithOnConflict("app_permissions", null, ContentValues().apply {
                put("install_id", installId)
                put("app_id", appId)
                put("permission", permissionName)
                put("requested", backupLong(permission, "requested", default = 1L))
                put("approved", backupLong(permission, "approved", default = 0L))
                putNullable("approved_at", backupString(permission, "approved_at", "approvedAt"))
                putNullable("reason", backupString(permission, "reason"))
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppStorage(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val storage = backupObjectAt(rows, index, "appStorage")
            val appId = requiredBackupString(storage, "app_id", "appId")
            val key = requiredBackupString(storage, "key")
            val inserted = db.insertWithOnConflict("app_storage", null, ContentValues().apply {
                put("app_id", appId)
                put("key", key)
                put("value_json", backupJsonText(storage, stringKeys = listOf("value_json", "valueJson"), valueKeys = listOf("value"), fallback = "null"))
                put("updated_at", backupString(storage, "updated_at", "updatedAt") ?: createdAt)
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppMigrations(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val migration = backupObjectAt(rows, index, "appMigrations")
            val migrationId = requiredBackupString(migration, "migration_id", "migrationId")
            val appId = requiredBackupString(migration, "app_id", "appId")
            val inserted = db.insertWithOnConflict("app_migrations", null, ContentValues().apply {
                put("migration_id", migrationId)
                put("app_id", appId)
                put("from_data_version", backupLong(migration, "from_data_version", "fromDataVersion", default = 1L))
                put("to_data_version", backupLong(migration, "to_data_version", "toDataVersion", default = 1L))
                put("migration_json", backupJsonText(migration, stringKeys = listOf("migration_json", "migrationJson"), valueKeys = listOf("migration"), fallback = "{}"))
                put("content_hash", backupString(migration, "content_hash", "contentHash") ?: "")
                put("created_at", backupString(migration, "created_at", "createdAt") ?: createdAt)
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun importAppInstallReports(db: SQLiteDatabase, rows: JSONArray, createdAt: String) {
        for (index in 0 until rows.length()) {
            val report = backupObjectAt(rows, index, "appInstallReports")
            val reportId = requiredBackupString(report, "report_id", "reportId")
            val appId = requiredBackupString(report, "app_id", "appId")
            val inserted = db.insertWithOnConflict("app_install_reports", null, ContentValues().apply {
                put("report_id", reportId)
                put("app_id", appId)
                putNullable("install_id", backupString(report, "install_id", "installId"))
                put("status", backupString(report, "status") ?: "accepted")
                putNullable("validation_json", backupJsonText(report, stringKeys = listOf("validation_json", "validationJson"), valueKeys = listOf("validation"), fallback = null))
                putNullable("security_json", backupJsonText(report, stringKeys = listOf("security_json", "securityJson"), valueKeys = listOf("security"), fallback = null))
                putNullable("permissions_json", backupJsonText(report, stringKeys = listOf("permissions_json", "permissionsJson"), valueKeys = listOf("permissions"), fallback = null))
                putNullable("compatibility_json", backupJsonText(report, stringKeys = listOf("compatibility_json", "compatibilityJson"), valueKeys = listOf("compatibility"), fallback = null))
                putNullable("smoke_test_json", backupJsonText(report, stringKeys = listOf("smoke_test_json", "smokeTestJson"), valueKeys = listOf("smokeTest"), fallback = null))
                putNullable("content_hash", backupString(report, "content_hash", "contentHash"))
                put("created_at", backupString(report, "created_at", "createdAt") ?: createdAt)
            }, SQLiteDatabase.CONFLICT_REPLACE)
            if (inserted < 0) throwBackupImportFailed()
        }
    }

    private fun queryRowsJson(table: String, args: JSONObject, filterColumn: String): JSONObject {
        val appId = args.optString("appId").ifBlank { null }
        return JSONObject(mapOf("rows" to tableRows(table, if (appId == null) null else filterColumn, appId)))
    }

    private fun requiredBackupArray(document: JSONObject, key: String): JSONArray =
        document.optJSONArray(key)
            ?: throw ControlCommandException(400, "invalid_request", "Backup import document is missing required arrays")

    private fun backupObjectAt(rows: JSONArray, index: Int, name: String): JSONObject =
        rows.optJSONObject(index)
            ?: throw ControlCommandException(400, "invalid_request", "Backup import $name row must be an object")

    private fun requiredBackupString(row: JSONObject, vararg keys: String): String =
        backupString(row, *keys)
            ?: throw ControlCommandException(400, "invalid_request", "Backup import document is missing required fields")

    private fun backupString(row: JSONObject?, vararg keys: String): String? {
        if (row == null) return null
        for (key in keys) {
            if (!row.has(key) || row.isNull(key)) continue
            val value = row.opt(key)
            if (value is String) return value.ifBlank { null }
            if (value is JSONObject || value is JSONArray || value == JSONObject.NULL) continue
            return value.toString().ifBlank { null }
        }
        return null
    }

    private fun backupLong(row: JSONObject, vararg keys: String, default: Long): Long {
        for (key in keys) {
            if (!row.has(key) || row.isNull(key)) continue
            return when (val value = row.opt(key)) {
                is Number -> value.toLong()
                is Boolean -> if (value) 1L else 0L
                is String -> value.toLongOrNull() ?: default
                else -> default
            }
        }
        return default
    }

    private fun backupJsonText(row: JSONObject, stringKeys: List<String>, valueKeys: List<String>, fallback: String?): String? {
        for (key in stringKeys) {
            if (!row.has(key) || row.isNull(key)) continue
            val value = row.opt(key)
            return if (value is String) value else jsonString(value)
        }
        for (key in valueKeys) {
            if (!row.has(key) || row.isNull(key)) continue
            return jsonString(row.opt(key))
        }
        return fallback
    }

    private fun ContentValues.putNullable(key: String, value: String?) {
        if (value == null) putNull(key) else put(key, value)
    }

    private fun throwBackupImportFailed(): Nothing =
        throw ControlCommandException(400, "sqlite_error", "Backup import could not be completed")

    private fun coreEventRowsJson(appId: String): JSONArray =
        rowsWithParsedJsonField(tableRows("core_events", "app_id", appId), "event_json", "event")

    private fun coreActionRowsJson(appId: String): JSONArray =
        rowsWithParsedJsonField(tableRows("core_actions", "app_id", appId), "action_json", "action")

    private fun rowsWithParsedJsonField(rows: JSONArray, jsonColumn: String, parsedKey: String): JSONArray {
        val enriched = JSONArray()
        for (index in 0 until rows.length()) {
            val row = rows.getJSONObject(index)
            val copy = JSONObject(row.toString())
            copy.put(parsedKey, parseJsonValue(row.optString(jsonColumn)))
            enriched.put(copy)
        }
        return enriched
    }

    private fun tableRows(table: String, filterColumn: String? = null, filterValue: String? = null): JSONArray {
        require(safeTables.contains(table)) { "Unsafe table requested" }
        if (filterColumn != null) require(safeFilterColumns.contains(filterColumn)) { "Unsafe filter column requested" }
        val selection = if (filterColumn != null && filterValue != null) "$filterColumn = ?" else null
        val selectionArgs = if (selection == null) null else arrayOf(filterValue)
        database.readableDatabase.query(table, null, selection, selectionArgs, null, null, null, "100").use { cursor ->
            return cursor.toJsonRows()
        }
    }

    private fun Cursor.toJsonRows(): JSONArray {
        val rows = JSONArray()
        while (moveToNext()) {
            val row = JSONObject()
            for (index in 0 until columnCount) {
                val name = getColumnName(index)
                row.put(name, jsonValue(index))
            }
            rows.put(row)
        }
        return rows
    }

    private fun Cursor.nullableString(index: Int): Any =
        if (getType(index) == Cursor.FIELD_TYPE_NULL) JSONObject.NULL else getString(index)

    private fun Cursor.jsonValue(index: Int): Any =
        when (getType(index)) {
            Cursor.FIELD_TYPE_NULL -> JSONObject.NULL
            Cursor.FIELD_TYPE_INTEGER -> getLong(index)
            Cursor.FIELD_TYPE_FLOAT -> getDouble(index)
            Cursor.FIELD_TYPE_BLOB -> Base64.encodeToString(getBlob(index), Base64.NO_WRAP)
            else -> getString(index)
        }

    private fun insertControlSession() {
        val values = ContentValues().apply {
            put("control_session_id", controlSessionId)
            put("target", "android")
            putNull("runtime_session_id")
            put("actor", "codex")
            put("token_hash", sha256Hex(token))
            put("started_at", Instant.now().toString())
            put("status", "running")
            put("metadata_json", JSONObject(mapOf("port" to port, "tokenPath" to tokenPath.absolutePath, "kind" to "listener")).toString())
        }
        database.writableDatabase.insertWithOnConflict("control_sessions", null, values, SQLiteDatabase.CONFLICT_REPLACE)
    }

    private fun finishControlSession() {
        database.writableDatabase.update(
            "control_sessions",
            ContentValues().apply {
                put("ended_at", Instant.now().toString())
                put("status", "ended")
            },
            "control_session_id = ?",
            arrayOf(controlSessionId),
        )
    }

    private fun audit(
        path: String,
        tool: String,
        httpMethod: String,
        decision: String,
        errorCode: String?,
        argsJson: JSONObject?,
        resultJson: JSONObject?,
        durationMs: Long,
    ) {
        val values = ContentValues().apply {
            put("command_id", "control_command_android_${UUID.randomUUID().toString().lowercase()}")
            put("control_session_id", controlSessionId)
            putNull("runtime_session_id")
            put("tool", tool)
            put("http_method", httpMethod)
            put("path", path)
            put("decision", decision)
            if (errorCode == null) putNull("error_code") else put("error_code", errorCode)
            if (argsJson == null) putNull("args_json") else put("args_json", argsJson.toString())
            if (resultJson == null) putNull("result_json") else put("result_json", resultJson.toString())
            if (errorCode == null) putNull("error_json") else put("error_json", JSONObject(mapOf("code" to errorCode)).toString())
            put("created_at", Instant.now().toString())
            put("duration_ms", durationMs)
        }
        database.writableDatabase.insert("control_commands", null, values)
    }

    private fun parseSessionRoute(path: String): SessionRoute? {
        val prefix = "/control/sessions/"
        if (!path.startsWith(prefix)) return null
        val segments = path.removePrefix(prefix).split('/').filter { it.isNotBlank() }
        if (segments.isEmpty()) return null
        return SessionRoute(segments[0], segments.getOrElse(1) { "end" })
    }

    private fun controlToolForPath(path: String): String = when {
        path == "/health" -> "platform.health"
        path == "/control/command" -> "control.command"
        path.startsWith("/control/sessions") -> "control.sessions"
        else -> "control.unknown"
    }

    private fun requiredString(args: JSONObject, key: String): String =
        args.optString(key).ifBlank { throw ControlCommandException(400, "invalid_request", "Control command requires $key") }

    private fun optionalString(args: JSONObject, key: String): String? =
        args.optString(key).ifBlank { null }

    private fun requiredToolString(args: JSONObject, key: String, message: String): String =
        args.optString(key).ifBlank { throw ControlCommandException(400, "invalid_request", message) }

    private fun requiredStorageString(args: JSONObject, key: String, message: String): String =
        args.optString(key).ifBlank { throw ControlCommandException(400, "invalid_request", message) }

    private fun typedStringOrNull(args: JSONObject, key: String, message: String): String? {
        if (!args.has(key)) return null
        val value = args.opt(key)
        if (value !is String || value.isBlank()) {
            throw ControlCommandException(400, "invalid_request", message)
        }
        return value
    }

    private fun writeControlTokenFile() {
        context.openFileOutput(tokenPath.name, Context.MODE_PRIVATE).use { output ->
            output.write("$token\n".toByteArray(Charsets.UTF_8))
        }
    }

    private fun readHttpLine(input: BufferedInputStream): String? {
        val bytes = mutableListOf<Byte>()
        while (true) {
            val next = input.read()
            if (next == -1) return if (bytes.isEmpty()) null else bytes.toByteArray().toString(Charsets.UTF_8)
            if (next == '\n'.code) break
            if (next != '\r'.code) bytes.add(next.toByte())
        }
        return bytes.toByteArray().toString(Charsets.UTF_8)
    }

    private fun readBody(input: BufferedInputStream, length: Int): String {
        if (length <= 0) return ""
        val buffer = ByteArray(length)
        var offset = 0
        while (offset < length) {
            val count = input.read(buffer, offset, length - offset)
            if (count < 0) break
            offset += count
        }
        return buffer.copyOf(offset).toString(Charsets.UTF_8)
    }

    private fun writeJson(socket: Socket, status: Int, body: JSONObject) {
        val payload = body.toString().toByteArray(Charsets.UTF_8)
        val statusText = when (status) {
            200 -> "OK"
            400 -> "Bad Request"
            401 -> "Unauthorized"
            404 -> "Not Found"
            else -> "Internal Server Error"
        }
        val headers = (
            "HTTP/1.1 $status $statusText\r\n" +
                "Content-Type: application/json\r\n" +
                "Content-Length: ${payload.size}\r\n" +
                "Connection: close\r\n\r\n"
            ).toByteArray(Charsets.UTF_8)
        socket.getOutputStream().use { output ->
            output.write(headers)
            output.write(payload)
        }
    }

    private fun jsonBodyOrNull(body: String): JSONObject? = try {
        if (body.isBlank()) null else JSONObject(body)
    } catch (_: Exception) {
        null
    }

    private fun parseJsonObject(text: String?): JSONObject? = try {
        if (text.isNullOrBlank()) null else JSONObject(text)
    } catch (_: Exception) {
        null
    }

    private fun parseJsonValue(text: String?): Any {
        if (text.isNullOrBlank()) return JSONObject.NULL
        return try {
            JSONObject(text)
        } catch (_: Exception) {
            try {
                JSONArray(text)
            } catch (_: Exception) {
                JSONObject.NULL
            }
        }
    }

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

    private fun Cursor.nullableStringValue(index: Int): String? =
        if (isNull(index)) null else getString(index)

    private fun jsonValuesEqual(left: Any?, right: Any?): Boolean {
        val normalizedLeft = left ?: JSONObject.NULL
        val normalizedRight = right ?: JSONObject.NULL
        if (normalizedLeft == JSONObject.NULL && normalizedRight == JSONObject.NULL) return true
        if (normalizedLeft is JSONObject && normalizedRight is JSONObject) {
            val leftKeys = normalizedLeft.keys().asSequence().toSet()
            val rightKeys = normalizedRight.keys().asSequence().toSet()
            return leftKeys == rightKeys && leftKeys.all { key -> jsonValuesEqual(normalizedLeft.opt(key), normalizedRight.opt(key)) }
        }
        if (normalizedLeft is JSONArray && normalizedRight is JSONArray) {
            if (normalizedLeft.length() != normalizedRight.length()) return false
            return (0 until normalizedLeft.length()).all { index -> jsonValuesEqual(normalizedLeft.opt(index), normalizedRight.opt(index)) }
        }
        if (normalizedLeft is Number && normalizedRight is Number) {
            return normalizedLeft.toDouble() == normalizedRight.toDouble()
        }
        return normalizedLeft == normalizedRight
    }

    private fun jsonMatchesSubset(actual: Any?, expected: Any?): Boolean {
        val normalizedExpected = expected ?: JSONObject.NULL
        if (normalizedExpected is JSONObject) {
            val actualObject = actual as? JSONObject ?: return false
            val keys = normalizedExpected.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                if (!actualObject.has(key)) return false
                if (!jsonMatchesSubset(actualObject.opt(key), normalizedExpected.opt(key))) return false
            }
            return true
        }
        return jsonValuesEqual(actual, normalizedExpected)
    }

    private fun controlOk(result: JSONObject): JSONObject = JSONObject(mapOf("ok" to true, "result" to result))

    private fun controlError(code: String, message: String): JSONObject = JSONObject(
        mapOf("ok" to false, "error" to JSONObject(mapOf("code" to code, "message" to message, "details" to JSONObject()))),
    )

    private data class HttpJsonResponse(val status: Int, val body: JSONObject)
    private data class SessionRoute(val sessionId: String, val action: String)
    private data class ActiveAppMetadata(val activeInstallId: String?, val activeVersion: String?, val dataVersion: Long)
    private class ControlCommandException(val status: Int, val code: String, message: String) : Exception(message)

    companion object {
        private const val tag = "TerraneAndroidDevControl"
        private const val androidRuntimeVersion = "0.1.0"
        private val knownBundledAppIds = listOf("notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab")
        private val snapshotTypes = setOf("bug-report", "pre-install", "pre-migration", "post-test", "golden", "manual", "debug-bundle")
        private val snapshotCompareSkipFields = setOf("createdAt", "snapshotId", "updated_at", "updatedAt")
        private val knownBridgeMethods = setOf(
            "storage.get",
            "storage.set",
            "storage.remove",
            "storage.list",
            "dialog.openFile",
            "dialog.saveFile",
            "notification.toast",
            "network.request",
            "core.step",
            "runtime.capabilities",
            "app.log",
        )
        private val safeTables = setOf(
            "apps",
            "app_versions",
            "app_files",
            "app_permissions",
            "app_installations",
            "app_storage",
            "app_install_reports",
            "app_migrations",
            "migration_runs",
            "runtime_sessions",
            "bridge_calls",
            "core_events",
            "core_actions",
            "runtime_snapshots",
            "control_sessions",
            "control_commands",
            "test_runs",
            "backup_exports",
        )
        private val safeFilterColumns = setOf("app_id", "control_session_id")

        private fun generateToken(): String {
            val bytes = ByteArray(32)
            SecureRandom().nextBytes(bytes)
            return Base64.encodeToString(bytes, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        }

        private fun sha256Hex(value: String): String {
            val digest = MessageDigest.getInstance("SHA-256").digest(value.toByteArray(Charsets.UTF_8))
            return digest.joinToString("") { "%02x".format(it.toInt() and 0xff) }
        }
    }
}
