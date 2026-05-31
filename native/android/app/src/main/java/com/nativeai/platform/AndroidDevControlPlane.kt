package com.nativeai.platform

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
        acceptThread = thread(name = "NativeAIAndroidDevControl", isDaemon = true) {
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
        Log.i(tag, "NATIVE_AI_ANDROID_CONTROL_READY port=$port tokenPath=${tokenPath.absolutePath}")
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
        "db.snapshot" -> dbSnapshotJson()
        "db.query_app_storage" -> queryRowsJson("app_storage", args, "app_id")
        "db.query_app_versions" -> queryRowsJson("app_versions", args, "app_id")
        "db.query_bridge_calls" -> queryRowsJson("bridge_calls", args, "app_id")
        "db.query_core_events" -> queryRowsJson("core_events", args, "app_id")
        "db.query_test_runs" -> queryRowsJson("test_runs", args, "app_id")
        else -> throw ControlCommandException(404, "unsupported_tool", "Unsupported Android dev control command")
    }

    private fun bridgeCommand(appId: String, method: String, params: JSONObject, id: String): JSONObject =
        bridge.handleControlBridgeCall(appId = appId, method = method, params = params, id = id)

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

    private fun runtimeEventLogJson(appId: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("bridgeCalls", tableRows("bridge_calls", "app_id", appId))
        .put("coreEvents", tableRows("core_events", "app_id", appId))
        .put("coreActions", tableRows("core_actions", "app_id", appId))

    private fun runtimeConsoleLogsJson(appId: String): JSONObject = JSONObject()
        .put("appId", appId)
        .put("logs", consoleLogRows(appId))

    private fun consoleLogRows(appId: String): JSONArray {
        val rows = JSONArray()
        database.readableDatabase.rawQuery(
            "SELECT bridge_call_id, app_id, params_json, error_json, created_at FROM bridge_calls WHERE app_id = ? AND method = 'app.log' ORDER BY created_at LIMIT 100",
            arrayOf(appId),
        ).use { cursor ->
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
                        .put("error", cursor.getString(3)?.let { parseJsonObject(it) ?: it } ?: JSONObject.NULL)
                        .put("createdAt", cursor.getString(4)),
                )
            }
        }
        return rows
    }

    private fun scalarLong(sql: String, selectionArgs: Array<String>): Long {
        database.readableDatabase.rawQuery(sql, selectionArgs).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getLong(0) else 0L
        }
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
                    "runtime.capabilities",
                    "runtime.call_bridge",
                    "runtime.core_step",
                    "runtime.storage_get",
                    "runtime.storage_set",
                    "runtime.assert_storage",
                    "runtime.resource_usage",
                    "runtime.event_log",
                    "runtime.console_logs",
                    "db.snapshot",
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
            "app_storage" to tableRows("app_storage"),
            "runtime_sessions" to tableRows("runtime_sessions"),
            "bridge_calls" to tableRows("bridge_calls"),
            "core_events" to tableRows("core_events"),
            "core_actions" to tableRows("core_actions"),
            "control_sessions" to tableRows("control_sessions"),
            "control_commands" to tableRows("control_commands"),
            "test_runs" to tableRows("test_runs"),
        ),
    )

    private fun queryRowsJson(table: String, args: JSONObject, filterColumn: String): JSONObject {
        val appId = args.optString("appId").ifBlank { null }
        return JSONObject(mapOf("rows" to tableRows(table, if (appId == null) null else filterColumn, appId)))
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
                when (getType(index)) {
                    Cursor.FIELD_TYPE_NULL -> row.put(name, JSONObject.NULL)
                    Cursor.FIELD_TYPE_INTEGER -> row.put(name, getLong(index))
                    Cursor.FIELD_TYPE_FLOAT -> row.put(name, getDouble(index))
                    Cursor.FIELD_TYPE_BLOB -> row.put(name, Base64.encodeToString(getBlob(index), Base64.NO_WRAP))
                    else -> row.put(name, getString(index))
                }
            }
            rows.put(row)
        }
        return rows
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

    private fun requiredStorageString(args: JSONObject, key: String, message: String): String =
        args.optString(key).ifBlank { throw ControlCommandException(400, "invalid_request", message) }

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

    private fun controlOk(result: JSONObject): JSONObject = JSONObject(mapOf("ok" to true, "result" to result))

    private fun controlError(code: String, message: String): JSONObject = JSONObject(
        mapOf("ok" to false, "error" to JSONObject(mapOf("code" to code, "message" to message, "details" to JSONObject()))),
    )

    private data class HttpJsonResponse(val status: Int, val body: JSONObject)
    private data class SessionRoute(val sessionId: String, val action: String)
    private class ControlCommandException(val status: Int, val code: String, message: String) : Exception(message)

    companion object {
        private const val tag = "NativeAIAndroidDevControl"
        private val safeTables = setOf(
            "apps",
            "app_versions",
            "app_storage",
            "runtime_sessions",
            "bridge_calls",
            "core_events",
            "core_actions",
            "control_sessions",
            "control_commands",
            "test_runs",
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
