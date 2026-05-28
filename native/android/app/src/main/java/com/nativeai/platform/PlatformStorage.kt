package com.nativeai.platform

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import org.json.JSONArray
import org.json.JSONObject

class PlatformStorage(context: Context) : SQLiteOpenHelper(context, "platform.sqlite", null, 1) {
    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key))",
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) = Unit

    fun get(request: BridgeRequest): String {
        val key = request.params.optString("key")
        if (key.isBlank()) return BridgeResponse.failure(request.id, "invalid_request", "storage.get requires key").toString()
        if (!key.startsWith(request.context.storagePrefix)) return storagePrefixFailure(request, key).toString()

        readableDatabase.rawQuery(
            "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?",
            arrayOf(request.context.appId, key),
        ).use { cursor ->
            if (cursor.moveToFirst()) {
                return BridgeResponse.success(request.id, JSONObject(mapOf("value" to decodeJson(cursor.getString(0))))).toString()
            }
        }
        return BridgeResponse.success(
            request.id,
            JSONObject().put("value", request.params.opt("defaultValue") ?: JSONObject.NULL),
        ).toString()
    }

    fun set(request: BridgeRequest): String {
        val key = request.params.optString("key")
        if (key.isBlank()) return BridgeResponse.failure(request.id, "invalid_request", "storage.set requires key").toString()
        if (!key.startsWith(request.context.storagePrefix)) return storagePrefixFailure(request, key).toString()

        val valueJson = encodeJson(request.params.opt("value"))
        val values = ContentValues().apply {
            put("app_id", request.context.appId)
            put("key", key)
            put("value_json", valueJson)
            put("updated_at", java.time.Instant.now().toString())
        }
        writableDatabase.insertWithOnConflict("app_storage", null, values, SQLiteDatabase.CONFLICT_REPLACE)
        return BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true, "bytesWritten" to valueJson.toByteArray().size))).toString()
    }

    fun remove(request: BridgeRequest): String {
        val key = request.params.optString("key")
        if (key.isBlank()) return BridgeResponse.failure(request.id, "invalid_request", "storage.remove requires key").toString()
        if (!key.startsWith(request.context.storagePrefix)) return storagePrefixFailure(request, key).toString()
        writableDatabase.delete("app_storage", "app_id = ? AND key = ?", arrayOf(request.context.appId, key))
        return BridgeResponse.success(request.id, JSONObject(mapOf("ok" to true))).toString()
    }

    fun list(request: BridgeRequest): String {
        val prefix = request.params.optString("prefix")
        if (prefix.isBlank()) return BridgeResponse.failure(request.id, "invalid_request", "storage.list requires prefix").toString()
        if (!prefix.startsWith(request.context.storagePrefix)) return storagePrefixFailure(request, prefix).toString()
        val keys = JSONArray()
        readableDatabase.rawQuery(
            "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key",
            arrayOf(request.context.appId, "$prefix%"),
        ).use { cursor ->
            while (cursor.moveToNext()) keys.put(cursor.getString(0))
        }
        return BridgeResponse.success(request.id, JSONObject(mapOf("keys" to keys))).toString()
    }

    private fun storagePrefixFailure(request: BridgeRequest, key: String): JSONObject = BridgeResponse.failure(
        request.id,
        "permission_denied",
        "Storage key must begin with ${request.context.storagePrefix}",
        JSONObject(mapOf("key" to key, "prefix" to request.context.storagePrefix, "appId" to request.context.appId)),
    )

    private fun encodeJson(value: Any?): String = when (value) {
        null -> "null"
        JSONObject.NULL -> "null"
        is JSONObject -> value.toString()
        is JSONArray -> value.toString()
        is String -> JSONObject.quote(value)
        is Number -> value.toString()
        is Boolean -> value.toString()
        else -> JSONObject.quote(value.toString())
    }

    private fun decodeJson(text: String): Any = try {
        when {
            text.startsWith("{") -> JSONObject(text)
            text.startsWith("[") -> JSONArray(text)
            text == "null" -> JSONObject.NULL
            else -> text
        }
    } catch (_: Exception) {
        JSONObject.NULL
    }
}
