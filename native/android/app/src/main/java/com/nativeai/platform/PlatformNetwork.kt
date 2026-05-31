package com.nativeai.platform

import okhttp3.Headers
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.SocketTimeoutException
import java.net.URL
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

private val plainTextMediaType = "text/plain".toMediaType()

class PlatformNetwork(private val database: PlatformDatabase? = null) {
    fun request(request: BridgeRequest): String {
        val urlText = request.params.optString("url", "")
        val url = try {
            URL(urlText)
        } catch (_: Exception) {
            return BridgeResponse.failure(request.id, "invalid_request", "network.request requires an absolute url").toString()
        }
        val origin = origin(url)
            ?: return BridgeResponse.failure(request.id, "invalid_request", "network.request requires an http or https url").toString()
        if (request.context.denyPrivateNetwork && isPrivateNetworkHost(url.host)) {
            return BridgeResponse.failure(request.id, "network_policy_denied", "network.request private network targets are denied").toString()
        }
        if (request.params.has("credentials") && !request.params.isNull("credentials")) {
            return BridgeResponse.failure(request.id, "network_policy_denied", "network.request credentials are not allowed").toString()
        }
        val method = request.params.optString("method", "GET").uppercase(Locale.US)
        val headers = parseHeaders(request)
            ?: return BridgeResponse.failure(request.id, "invalid_request", "network.request headers must be strings").toString()
        val bodyResult = parseBody(request)
        if (bodyResult is NetworkBody.Invalid) {
            return BridgeResponse.failure(request.id, "invalid_request", "network.request body must be a string or null").toString()
        }
        val body = (bodyResult as NetworkBody.Valid).bytes

        val path = path(url)
        val rule = request.context.networkPolicy.firstOrNull { it.allows(origin, method, path, headers.keys) }
            ?: return BridgeResponse.failure(request.id, "network_policy_denied", "network.request is not allowed by manifest.networkPolicy").toString()
        if (body != null && body.size > rule.maxRequestBytes) {
            return BridgeResponse.failure(request.id, "network_policy_denied", "network.request body exceeds manifest.networkPolicy maxRequestBytes").toString()
        }
        val requestedTimeoutMs = when (val parsedTimeout = requestedTimeoutMs(request.params)) {
            is NetworkTimeout.Invalid -> {
                return BridgeResponse.failure(
                    request.id,
                    "invalid_request",
                    "network.request timeoutMs must be a positive integer",
                    JSONObject(mapOf("timeoutMs" to parsedTimeout.value)),
                ).toString()
            }
            NetworkTimeout.Missing -> null
            is NetworkTimeout.Valid -> parsedTimeout.value
        }
        val effectiveTimeoutMs = effectiveTimeoutMs(rule, requestedTimeoutMs)
        val mocked = mockedNetworkResponse(request, rule, urlText, method, effectiveTimeoutMs)
        if (mocked != null) {
            return mocked
        }

        return performRequestOffMainThread(request, url, method, headers, body, rule, effectiveTimeoutMs, request.context.networkPolicy, request.context.denyPrivateNetwork)
    }

    private fun performRequestOffMainThread(
        request: BridgeRequest,
        initialUrl: URL,
        method: String,
        headers: Map<String, String>,
        body: ByteArray?,
        rule: NetworkPolicyRule,
        effectiveTimeoutMs: Int,
        policy: List<NetworkPolicyRule>,
        denyPrivateNetwork: Boolean,
    ): String {
        val result = AtomicReference<String>()
        val latch = CountDownLatch(1)
        Thread {
            try {
                result.set(performRequest(request, initialUrl, method, headers, body, rule, effectiveTimeoutMs, policy, denyPrivateNetwork))
            } catch (error: Exception) {
                result.set(BridgeResponse.failure(request.id, "network_error", error.localizedMessage ?: "network.request failed").toString())
            } finally {
                latch.countDown()
            }
        }.start()
        val completed = latch.await((effectiveTimeoutMs + 1_000).toLong(), TimeUnit.MILLISECONDS)
        return if (completed) {
            result.get()
        } else {
            timeoutFailure(request, effectiveTimeoutMs)
        }
    }

    private fun performRequest(
        request: BridgeRequest,
        initialUrl: URL,
        method: String,
        headers: Map<String, String>,
        body: ByteArray?,
        rule: NetworkPolicyRule,
        effectiveTimeoutMs: Int,
        policy: List<NetworkPolicyRule>,
        denyPrivateNetwork: Boolean,
    ): String {
        var currentUrl = initialUrl
        val client = OkHttpClient.Builder()
            .followRedirects(false)
            .callTimeout(effectiveTimeoutMs.toLong(), TimeUnit.MILLISECONDS)
            .connectTimeout(effectiveTimeoutMs.toLong(), TimeUnit.MILLISECONDS)
            .readTimeout(effectiveTimeoutMs.toLong(), TimeUnit.MILLISECONDS)
            .writeTimeout(effectiveTimeoutMs.toLong(), TimeUnit.MILLISECONDS)
            .build()
        repeat(6) { redirectCount ->
            try {
                client.newCall(okHttpRequest(currentUrl, method, headers, body)).execute().use { response ->
                    val status = response.code
                    val location = response.header("Location")
                    if (status in 300..399 && location != null) {
                        val nextUrl = URL(currentUrl, location)
                        val nextOrigin = origin(nextUrl)
                        if (nextOrigin == null || (denyPrivateNetwork && isPrivateNetworkHost(nextUrl.host)) || policy.none { it.allows(nextOrigin, method, path(nextUrl), headers.keys) }) {
                            return BridgeResponse.failure(request.id, "network_policy_denied", "network.request redirect is not allowed by manifest.networkPolicy").toString()
                        }
                        if (redirectCount == 5) {
                            return BridgeResponse.failure(request.id, "network_error", "network.request exceeded redirect limit").toString()
                        }
                        currentUrl = nextUrl
                        return@repeat
                    }

                    val data = readResponseBytes(response, rule.maxResponseBytes)
                    if (data == null) {
                        return BridgeResponse.failure(request.id, "network_policy_denied", "network.response exceeds manifest.networkPolicy maxResponseBytes").toString()
                    }
                    val result = JSONObject(
                        mapOf(
                            "status" to status,
                            "headers" to responseHeaders(response.headers),
                            "bodyText" to data.toString(Charsets.UTF_8),
                        ),
                    )
                    return BridgeResponse.success(request.id, result).toString()
                }
            } catch (_: SocketTimeoutException) {
                return timeoutFailure(request, effectiveTimeoutMs)
            } catch (error: Exception) {
                return BridgeResponse.failure(request.id, "network_error", error.localizedMessage ?: "network.request failed").toString()
            }

        }
        return BridgeResponse.failure(request.id, "network_error", "network.request failed").toString()
    }

    private fun parseHeaders(request: BridgeRequest): Map<String, String>? {
        if (!request.params.has("headers") || request.params.isNull("headers")) return emptyMap()
        val raw = request.params.optJSONObject("headers") ?: return null
        val headers = mutableMapOf<String, String>()
        raw.keys().forEach { name ->
            val value = raw.opt(name)
            if (value !is String) return null
            headers[name.lowercase(Locale.US)] = value
        }
        return headers
    }

    private fun parseBody(request: BridgeRequest): NetworkBody {
        if (!request.params.has("body") || request.params.isNull("body")) return NetworkBody.Valid(null)
        val value = request.params.opt("body")
        if (value !is String) return NetworkBody.Invalid
        return NetworkBody.Valid(value.toByteArray(Charsets.UTF_8))
    }

    private fun requestedTimeoutMs(params: JSONObject): NetworkTimeout {
        if (!params.has("timeoutMs")) return NetworkTimeout.Missing
        val value = params.opt("timeoutMs") ?: JSONObject.NULL
        if (value !is Number) return NetworkTimeout.Invalid(value)
        val doubleValue = value.toDouble()
        val longValue = value.toLong()
        if (!java.lang.Double.isFinite(doubleValue) || doubleValue <= 0 || doubleValue != longValue.toDouble() || longValue > Int.MAX_VALUE) {
            return NetworkTimeout.Invalid(value)
        }
        return NetworkTimeout.Valid(longValue.toInt())
    }

    private fun effectiveTimeoutMs(rule: NetworkPolicyRule, requestedTimeoutMs: Int?): Int =
        requestedTimeoutMs?.let { minOf(rule.timeoutMs, it) } ?: rule.timeoutMs

    private fun mockedNetworkResponse(
        request: BridgeRequest,
        rule: NetworkPolicyRule,
        url: String,
        method: String,
        effectiveTimeoutMs: Int,
    ): String? {
        val mock = findNetworkMock(request, method, url) ?: return null
        val delayMs = positiveInteger(mock.opt("delayMs"))
        if (delayMs != null && delayMs > effectiveTimeoutMs) {
            return BridgeResponse.failure(
                request.id,
                "timeout",
                "network.request timed out",
                JSONObject(mapOf("timeoutMs" to effectiveTimeoutMs, "delayMs" to delayMs)),
            ).toString()
        }
        if (mockResponseBytes(mock) > rule.maxResponseBytes) {
            return BridgeResponse.failure(request.id, "network_policy_denied", "network.response exceeds manifest.networkPolicy maxResponseBytes").toString()
        }
        return BridgeResponse.success(request.id, payloadWithoutDelay(mock)).toString()
    }

    private fun findNetworkMock(request: BridgeRequest, method: String, url: String): JSONObject? {
        val db = database?.readableDatabase ?: return null
        val sessionId = runtimeSessionId(request)
        db.rawQuery(
            "SELECT response_json, url_pattern FROM network_mocks " +
                "WHERE enabled = 1 AND method = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) " +
                "ORDER BY created_at DESC LIMIT 100",
            arrayOf(method, request.context.appId, sessionId),
        ).use { cursor ->
            while (cursor.moveToNext()) {
                val pattern = cursor.getString(1)
                if (!urlMatches(pattern, url)) continue
                val mock = parseJsonObject(cursor.getString(0))
                if (mock != null) return mock
            }
        }
        return null
    }

    private fun runtimeSessionId(request: BridgeRequest): String =
        "runtime_android_${request.context.appId}_${request.context.mountToken ?: "native"}"

    private fun urlMatches(pattern: String?, url: String): Boolean {
        if (pattern == null) return false
        if (pattern == "*" || pattern == url) return true
        if (pattern.endsWith("*")) {
            return url.startsWith(pattern.dropLast(1))
        }
        return false
    }

    private fun positiveInteger(value: Any?): Int? {
        if (value !is Number) return null
        val doubleValue = value.toDouble()
        val longValue = value.toLong()
        if (!java.lang.Double.isFinite(doubleValue) || doubleValue <= 0 || doubleValue != longValue.toDouble() || longValue > Int.MAX_VALUE) {
            return null
        }
        return longValue.toInt()
    }

    private fun mockResponseBytes(mock: JSONObject): Int {
        if (mock.has("bodyText")) {
            return jsonPayloadBytes(mock.opt("bodyText"))
        }
        if (mock.has("body")) {
            return jsonPayloadBytes(mock.opt("body"))
        }
        return 0
    }

    private fun jsonPayloadBytes(value: Any?): Int = when (value) {
        null, JSONObject.NULL -> 0
        is String -> value.toByteArray(Charsets.UTF_8).size
        is JSONObject, is JSONArray -> value.toString().toByteArray(Charsets.UTF_8).size
        is Number, is Boolean -> value.toString().toByteArray(Charsets.UTF_8).size
        else -> value.toString().toByteArray(Charsets.UTF_8).size
    }

    private fun payloadWithoutDelay(mock: JSONObject): JSONObject {
        val payload = JSONObject()
        val keys = mock.keys()
        while (keys.hasNext()) {
            val key = keys.next()
            if (key != "delayMs") {
                payload.put(key, mock.opt(key) ?: JSONObject.NULL)
            }
        }
        return payload
    }

    private fun parseJsonObject(text: String?): JSONObject? = try {
        if (text.isNullOrBlank()) null else JSONObject(text)
    } catch (_: Exception) {
        null
    }

    private fun timeoutFailure(request: BridgeRequest, timeoutMs: Int): String =
        BridgeResponse.failure(
            request.id,
            "timeout",
            "network.request timed out",
            JSONObject(mapOf("timeoutMs" to timeoutMs)),
        ).toString()

    private fun okHttpRequest(url: URL, method: String, headers: Map<String, String>, body: ByteArray?): Request {
        val builder = Request.Builder().url(url)
        headers.forEach { (name, value) -> builder.header(name, value) }
        builder.method(method, requestBodyFor(method, body))
        return builder.build()
    }

    private fun requestBodyFor(method: String, body: ByteArray?): RequestBody? {
        if (body != null) return body.toRequestBody(plainTextMediaType)
        return if (method in setOf("POST", "PUT", "PATCH")) {
            ByteArray(0).toRequestBody(plainTextMediaType)
        } else {
            null
        }
    }

    private fun readResponseBytes(response: Response, maxBytes: Int): ByteArray? {
        val stream = response.body?.byteStream()
        if (stream == null) return ByteArray(0)
        stream.use { input ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                out.write(buffer, 0, read)
                if (out.size() > maxBytes) return null
            }
            return out.toByteArray()
        }
    }

    private fun responseHeaders(responseHeaders: Headers): JSONObject {
        val headers = JSONObject()
        responseHeaders.names().forEach { name ->
            headers.put(name.lowercase(Locale.US), responseHeaders.values(name).joinToString(", "))
        }
        return headers
    }

    companion object {
        fun origin(url: URL): String? {
            val protocol = url.protocol.lowercase(Locale.US)
            if (protocol != "http" && protocol != "https") return null
            val host = url.host.lowercase(Locale.US)
            val port = url.port
            if (port > 0 && !(protocol == "http" && port == 80) && !(protocol == "https" && port == 443)) {
                return "$protocol://$host:$port"
            }
            return "$protocol://$host"
        }

        fun path(url: URL): String = url.path.ifEmpty { "/" }

        fun isPrivateNetworkHost(rawHost: String?): Boolean {
            var host = rawHost?.trim()?.lowercase(Locale.US) ?: return false
            if (host.startsWith("[") && host.endsWith("]")) {
                host = host.substring(1, host.length - 1)
            }
            host = host.substringBefore("%")
            if (host == "localhost" || host.endsWith(".localhost")) return true
            parseIpv4Host(host)?.let { return isPrivateIpv4(it) }
            if (host == "::1") return true
            if (host.startsWith("fc") || host.startsWith("fd")) return true
            if (host.startsWith("fe8") || host.startsWith("fe9") || host.startsWith("fea") || host.startsWith("feb")) return true
            if (host.startsWith("::ffff:")) return isPrivateIpv4MappedHost(host.removePrefix("::ffff:"))
            return false
        }

        private fun isPrivateIpv4MappedHost(tail: String): Boolean {
            parseIpv4Host(tail)?.let { return isPrivateIpv4(it) }
            val parts = tail.split(":")
            if (parts.size != 2) return false
            val high = parts[0].toIntOrNull(16) ?: return false
            val low = parts[1].toIntOrNull(16) ?: return false
            return isPrivateIpv4(listOf(
                (high shr 8) and 255,
                high and 255,
                (low shr 8) and 255,
                low and 255,
            ))
        }

        private fun parseIpv4Host(host: String): List<Int>? {
            val parts = host.split(".")
            if (parts.size != 4) return null
            return parts.map { part ->
                if (part.isEmpty() || part.any { !it.isDigit() }) return null
                val value = part.toIntOrNull() ?: return null
                if (value !in 0..255) return null
                value
            }
        }

        private fun isPrivateIpv4(octets: List<Int>): Boolean {
            if (octets.size != 4) return false
            val first = octets[0]
            val second = octets[1]
            return first == 0 ||
                first == 10 ||
                first == 127 ||
                (first == 100 && second in 64..127) ||
                (first == 169 && second == 254) ||
                (first == 172 && second in 16..31) ||
                (first == 192 && second == 168)
        }
    }
}

private sealed class NetworkBody {
    data class Valid(val bytes: ByteArray?) : NetworkBody()
    data object Invalid : NetworkBody()
}

private sealed class NetworkTimeout {
    data object Missing : NetworkTimeout()
    data class Valid(val value: Int) : NetworkTimeout()
    data class Invalid(val value: Any) : NetworkTimeout()
}

data class NetworkPolicyRule(
    val origin: String,
    val methods: Set<String>,
    val pathPrefix: String?,
    val allowedHeaders: Set<String>,
    val maxRequestBytes: Int,
    val maxResponseBytes: Int,
    val timeoutMs: Int,
) {
    fun allows(origin: String, method: String, path: String, headers: Set<String>): Boolean {
        if (this.origin != origin || !methods.contains(method)) return false
        if (pathPrefix != null && !path.startsWith(pathPrefix)) return false
        return headers.all { header ->
            val normalized = header.lowercase(Locale.US)
            normalized != "cookie" && normalized != "set-cookie" && allowedHeaders.contains(normalized)
        }
    }

    companion object {
        fun fromManifest(manifest: JSONObject): List<NetworkPolicyRule> {
            val allow = manifest.optJSONObject("networkPolicy")?.optJSONArray("allow") ?: return emptyList()
            return (0 until allow.length()).mapNotNull { index ->
                val raw = allow.optJSONObject(index) ?: return@mapNotNull null
                val origin = raw.optString("origin", "")
                if (origin.isBlank()) return@mapNotNull null
                NetworkPolicyRule(
                    origin = origin,
                    methods = raw.optJSONArray("methods").toStringSet { it.uppercase(Locale.US) },
                    pathPrefix = raw.optString("pathPrefix", "").ifBlank { null },
                    allowedHeaders = raw.optJSONArray("allowedHeaders").toStringSet { it.lowercase(Locale.US) },
                    maxRequestBytes = raw.optInt("maxRequestBytes", 0),
                    maxResponseBytes = raw.optInt("maxResponseBytes", 0),
                    timeoutMs = raw.optInt("timeoutMs", 10_000),
                )
            }
        }
    }
}

private fun org.json.JSONArray?.toStringSet(transform: (String) -> String): Set<String> {
    if (this == null) return emptySet()
    return (0 until length()).mapNotNull { index ->
        optString(index, "").takeIf { it.isNotBlank() }?.let(transform)
    }.toSet()
}
