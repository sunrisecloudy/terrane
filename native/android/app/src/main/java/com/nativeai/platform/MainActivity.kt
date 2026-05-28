package com.nativeai.platform

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewCompat
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.io.InputStream

class MainActivity : Activity() {
    private lateinit var webView: WebView
    private lateinit var bridge: NativeBridge
    private lateinit var assetLoader: WebViewAssetLoader
    private lateinit var activeSandboxContext: AppSandboxContext

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        activeSandboxContext = sandboxContextFromManifest("notes-lite")

        assetLoader = WebViewAssetLoader.Builder()
            .addPathHandler("/", AssetRootPathHandler(this))
            .build()

        bridge = NativeBridge(
            context = this,
            activeContext = { activeSandboxContext },
        )

        webView = WebView(this)
        webView.settings.javaScriptEnabled = true
        webView.settings.allowFileAccess = false
        webView.settings.allowContentAccess = false
        webView.webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                return assetLoader.shouldInterceptRequest(request.url)
            }
        }

        WebViewCompat.addWebMessageListener(
            webView,
            "NativeAIPlatformBridge",
            setOf("https://appassets.androidplatform.net"),
        ) { _, message, _, _, replyProxy ->
            replyProxy.postMessage(bridge.handle(message.data ?: "{}"))
        }

        setContentView(webView)
        webView.loadUrl("https://appassets.androidplatform.net/runtime/index.html")
    }

    private fun sandboxContextFromManifest(appId: String): AppSandboxContext {
        val manifest = loadExampleManifest(appId)
        val actualAppId = manifest.optString("id", appId)
        return AppSandboxContext(
            appId = actualAppId,
            storagePrefix = manifest.optString("storagePrefix", "$actualAppId:"),
            approvedPermissions = manifest.optJSONArray("permissions").toStringSet { it },
            networkPolicy = NetworkPolicyRule.fromManifest(manifest),
        )
    }

    private fun loadExampleManifest(appId: String): JSONObject {
        val path = "webapps/examples/$appId/manifest.json"
        return assets.open(path).bufferedReader(Charsets.UTF_8).use { reader ->
            JSONObject(reader.readText())
        }
    }
}

private class AssetRootPathHandler(private val context: Context) : WebViewAssetLoader.PathHandler {
    override fun handle(path: String): WebResourceResponse? {
        val assetPath = path.trimStart('/')
        if (!isAllowedAsset(assetPath)) return null
        val stream = openAsset(assetPath) ?: return null
        return WebResourceResponse(contentType(assetPath), "UTF-8", stream)
    }

    private fun openAsset(path: String): InputStream? = try {
        context.assets.open(path)
    } catch (_: IOException) {
        null
    }

    private fun isAllowedAsset(path: String): Boolean {
        if (path.isBlank() || path.contains("..") || path.contains('\\')) return false
        return path.startsWith("runtime/") || path.startsWith("webapps/examples/")
    }

    private fun contentType(path: String): String = when {
        path.endsWith(".html") -> "text/html"
        path.endsWith(".css") -> "text/css"
        path.endsWith(".js") -> "text/javascript"
        path.endsWith(".json") -> "application/json"
        else -> "text/plain"
    }
}

private fun JSONArray?.toStringSet(transform: (String) -> String): Set<String> {
    if (this == null) return emptySet()
    return (0 until length()).mapNotNull { index ->
        optString(index, "").takeIf { it.isNotBlank() }?.let(transform)
    }.toSet()
}
