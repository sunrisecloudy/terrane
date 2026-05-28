package com.nativeai.platform

import android.annotation.SuppressLint
import android.app.Activity
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewCompat

class MainActivity : Activity() {
    private lateinit var webView: WebView
    private lateinit var bridge: NativeBridge
    private lateinit var assetLoader: WebViewAssetLoader

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        assetLoader = WebViewAssetLoader.Builder()
            .addPathHandler("/assets/", WebViewAssetLoader.AssetsPathHandler(this))
            .build()

        bridge = NativeBridge(
            context = this,
            activeContext = {
                AppSandboxContext(
                    appId = "notes-lite",
                    storagePrefix = "notes-lite:",
                    approvedPermissions = setOf("storage.read", "storage.write", "notification.toast"),
                )
            },
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
        webView.loadUrl("https://appassets.androidplatform.net/assets/runtime/index.html")
    }
}
