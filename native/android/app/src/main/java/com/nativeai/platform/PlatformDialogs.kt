package com.nativeai.platform

import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream

class PlatformDialogs(private val activity: ComponentActivity) {
    private data class PendingOpen(val request: BridgeRequest, val respond: (String) -> Unit)
    private data class PendingSave(val request: BridgeRequest, val respond: (String) -> Unit)

    private var pendingOpen: PendingOpen? = null
    private var pendingManyOpen: PendingOpen? = null
    private var pendingSave: PendingSave? = null

    private val openDocument = activity.registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val pending = pendingOpen
        pendingOpen = null
        if (pending == null) return@registerForActivityResult
        if (uri == null) {
            pending.respond(cancelled(pending.request, "Open file was cancelled"))
            return@registerForActivityResult
        }
        pending.respond(openResult(pending.request, listOf(uri)))
    }

    private val openDocuments = activity.registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        val pending = pendingManyOpen
        pendingManyOpen = null
        if (pending == null) return@registerForActivityResult
        if (uris.isNullOrEmpty()) {
            pending.respond(cancelled(pending.request, "Open file was cancelled"))
            return@registerForActivityResult
        }
        pending.respond(openResult(pending.request, uris))
    }

    private val createDocument = activity.registerForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        val pending = pendingSave
        pendingSave = null
        if (pending == null) return@registerForActivityResult
        if (uri == null) {
            pending.respond(cancelled(pending.request, "Save file was cancelled"))
            return@registerForActivityResult
        }
        pending.respond(saveResult(pending.request, uri))
    }

    fun openFile(request: BridgeRequest, respond: (String) -> Unit) {
        activity.runOnUiThread {
            if (isBusy()) {
                respond(BridgeResponse.failure(request.id, "capability_unavailable", "Another file dialog is already open").toString())
                return@runOnUiThread
            }
            val types = acceptedTypes(request)
            try {
                if (request.params.optBoolean("multiple", false)) {
                    pendingManyOpen = PendingOpen(request, respond)
                    openDocuments.launch(types)
                } else {
                    pendingOpen = PendingOpen(request, respond)
                    openDocument.launch(types)
                }
            } catch (error: Exception) {
                pendingOpen = null
                pendingManyOpen = null
                respond(BridgeResponse.failure(request.id, "platform_unsupported", error.localizedMessage ?: "dialog.openFile is unavailable").toString())
            }
        }
    }

    fun saveFile(request: BridgeRequest, respond: (String) -> Unit) {
        activity.runOnUiThread {
            if (isBusy()) {
                respond(BridgeResponse.failure(request.id, "capability_unavailable", "Another file dialog is already open").toString())
                return@runOnUiThread
            }
            pendingSave = PendingSave(request, respond)
            try {
                createDocument.launch(request.params.optString("suggestedName", "output.txt"))
            } catch (error: Exception) {
                pendingSave = null
                respond(BridgeResponse.failure(request.id, "platform_unsupported", error.localizedMessage ?: "dialog.saveFile is unavailable").toString())
            }
        }
    }

    private fun isBusy(): Boolean = pendingOpen != null || pendingManyOpen != null || pendingSave != null

    private fun acceptedTypes(request: BridgeRequest): Array<String> {
        val accept = request.params.optJSONArray("accept") ?: return arrayOf("text/plain")
        val values = (0 until accept.length()).mapNotNull { index ->
            accept.optString(index, "").takeIf { it.isNotBlank() }
        }
        return values.ifEmpty { listOf("text/plain") }.toTypedArray()
    }

    private fun maxBytes(request: BridgeRequest): Int {
        val value = request.params.optLong("maxBytes", 1024 * 1024)
        return when {
            value <= 0 -> 0
            value > Int.MAX_VALUE -> Int.MAX_VALUE
            else -> value.toInt()
        }
    }

    private fun openResult(request: BridgeRequest, uris: List<Uri>): String {
        val files = JSONArray()
        for (uri in uris) {
            val bytes = readBounded(uri, maxBytes(request))
                ?: return BridgeResponse.failure(request.id, "quota_exceeded", "Selected file exceeds maxBytes").toString()
            files.put(
                JSONObject(
                    mapOf(
                        "name" to displayName(uri),
                        "mime" to mimeFor(uri, request),
                        "size" to bytes.size,
                        "text" to bytes.toString(Charsets.UTF_8),
                    ),
                ),
            )
        }
        return BridgeResponse.success(request.id, JSONObject(mapOf("files" to files))).toString()
    }

    private fun saveResult(request: BridgeRequest, uri: Uri): String {
        return try {
            activity.contentResolver.openOutputStream(uri)?.use { output ->
                output.write(request.params.optString("text", "").toByteArray(Charsets.UTF_8))
            } ?: return BridgeResponse.failure(request.id, "storage_error", "Selected file could not be opened for writing").toString()
            BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true))).toString()
        } catch (error: Exception) {
            BridgeResponse.failure(request.id, "storage_error", error.localizedMessage ?: "Could not write selected file").toString()
        }
    }

    private fun readBounded(uri: Uri, maxBytes: Int): ByteArray? {
        val stream = activity.contentResolver.openInputStream(uri) ?: return null
        stream.use { input ->
            val output = ByteArrayOutputStream()
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            while (true) {
                val read = input.read(buffer)
                if (read == -1) break
                if (output.size() + read > maxBytes) return null
                output.write(buffer, 0, read)
            }
            return output.toByteArray()
        }
    }

    private fun displayName(uri: Uri): String {
        activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (index >= 0) {
                    val name = cursor.getString(index)
                    if (!name.isNullOrBlank()) return name
                }
            }
        }
        return uri.lastPathSegment ?: "selected.txt"
    }

    private fun mimeFor(uri: Uri, request: BridgeRequest): String {
        val resolved = activity.contentResolver.getType(uri)
        if (!resolved.isNullOrBlank()) return resolved
        val accept = request.params.optJSONArray("accept")
        val first = accept?.optString(0, "") ?: ""
        return first.ifBlank { "text/plain" }
    }

    private fun cancelled(request: BridgeRequest, message: String): String =
        BridgeResponse.failure(request.id, "dialog_cancelled", message).toString()
}
