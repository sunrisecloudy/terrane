package com.nativeai.platform

import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class PlatformNetwork {
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

        val rule = request.context.networkPolicy.firstOrNull { it.allows(origin, method, headers.keys) }
            ?: return BridgeResponse.failure(request.id, "network_policy_denied", "network.request is not allowed by manifest.networkPolicy").toString()
        if (body != null && body.size > rule.maxRequestBytes) {
            return BridgeResponse.failure(request.id, "network_policy_denied", "network.request body exceeds manifest.networkPolicy maxRequestBytes").toString()
        }

        return performRequestOffMainThread(request, url, method, headers, body, rule, request.context.networkPolicy, request.context.denyPrivateNetwork)
    }

    private fun performRequestOffMainThread(
        request: BridgeRequest,
        initialUrl: URL,
        method: String,
        headers: Map<String, String>,
        body: ByteArray?,
        rule: NetworkPolicyRule,
        policy: List<NetworkPolicyRule>,
        denyPrivateNetwork: Boolean,
    ): String {
        val result = AtomicReference<String>()
        val latch = CountDownLatch(1)
        Thread {
            try {
                result.set(performRequest(request, initialUrl, method, headers, body, rule, policy, denyPrivateNetwork))
            } catch (error: Exception) {
                result.set(BridgeResponse.failure(request.id, "network_error", error.localizedMessage ?: "network.request failed").toString())
            } finally {
                latch.countDown()
            }
        }.start()
        val completed = latch.await((rule.timeoutMs + 1_000).toLong(), TimeUnit.MILLISECONDS)
        return if (completed) {
            result.get()
        } else {
            BridgeResponse.failure(request.id, "timeout", "network.request timed out").toString()
        }
    }

    private fun performRequest(
        request: BridgeRequest,
        initialUrl: URL,
        method: String,
        headers: Map<String, String>,
        body: ByteArray?,
        rule: NetworkPolicyRule,
        policy: List<NetworkPolicyRule>,
        denyPrivateNetwork: Boolean,
    ): String {
        var currentUrl = initialUrl
        repeat(6) { redirectCount ->
            val connection = (currentUrl.openConnection() as HttpURLConnection).apply {
                requestMethod = method
                instanceFollowRedirects = false
                connectTimeout = rule.timeoutMs
                readTimeout = rule.timeoutMs
                useCaches = false
                doInput = true
                headers.forEach { (name, value) -> setRequestProperty(name, value) }
                if (body != null) {
                    doOutput = true
                    outputStream.use { it.write(body) }
                }
            }

            try {
                val status = connection.responseCode
                val location = connection.getHeaderField("Location")
                if (status in 300..399 && location != null) {
                    val nextUrl = URL(currentUrl, location)
                    val nextOrigin = origin(nextUrl)
                    if (nextOrigin == null || (denyPrivateNetwork && isPrivateNetworkHost(nextUrl.host)) || policy.none { it.allows(nextOrigin, method, headers.keys) }) {
                        connection.disconnect()
                        return BridgeResponse.failure(request.id, "network_policy_denied", "network.request redirect is not allowed by manifest.networkPolicy").toString()
                    }
                    if (redirectCount == 5) {
                        connection.disconnect()
                        return BridgeResponse.failure(request.id, "network_error", "network.request exceeded redirect limit").toString()
                    }
                    connection.disconnect()
                    currentUrl = nextUrl
                    return@repeat
                }

                val data = readResponseBytes(connection, rule.maxResponseBytes)
                if (data == null) {
                    connection.disconnect()
                    return BridgeResponse.failure(request.id, "network_policy_denied", "network.response exceeds manifest.networkPolicy maxResponseBytes").toString()
                }
                val result = JSONObject(
                    mapOf(
                        "status" to status,
                        "headers" to responseHeaders(connection),
                        "bodyText" to data.toString(Charsets.UTF_8),
                    ),
                )
                connection.disconnect()
                return BridgeResponse.success(request.id, result).toString()
            } catch (_: SocketTimeoutException) {
                connection.disconnect()
                return BridgeResponse.failure(request.id, "timeout", "network.request timed out").toString()
            } catch (error: Exception) {
                connection.disconnect()
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

    private fun readResponseBytes(connection: HttpURLConnection, maxBytes: Int): ByteArray? {
        val stream = if (connection.responseCode >= 400) connection.errorStream else connection.inputStream
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

    private fun responseHeaders(connection: HttpURLConnection): JSONObject {
        val headers = JSONObject()
        connection.headerFields.forEach { (name, values) ->
            if (name != null && values != null && values.isNotEmpty()) {
                headers.put(name.lowercase(Locale.US), values.joinToString(", "))
            }
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

data class NetworkPolicyRule(
    val origin: String,
    val methods: Set<String>,
    val allowedHeaders: Set<String>,
    val maxRequestBytes: Int,
    val maxResponseBytes: Int,
    val timeoutMs: Int,
) {
    fun allows(origin: String, method: String, headers: Set<String>): Boolean {
        if (this.origin != origin || !methods.contains(method)) return false
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
