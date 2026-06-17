package com.terrane.platform

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.util.Log
import android.webkit.ConsoleMessage
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.io.InputStream

class MainActivity : ComponentActivity() {
    private val exampleAppIds = setOf("notes-lite", "task-workbench", "file-transformer", "api-dashboard", "core-replay-lab", "calendar-planner")
    private lateinit var webView: WebView
    private lateinit var bridge: NativeBridge
    private lateinit var dialogs: PlatformDialogs
    private lateinit var assetLoader: WebViewAssetLoader
    private var smokeProbe: AndroidSmokeProbe? = null
    private var devControlPlane: AndroidDevControlPlane? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        smokeProbe = AndroidSmokeProbe.fromIntent(intent)

        assetLoader = WebViewAssetLoader.Builder()
            .addPathHandler("/", AssetRootPathHandler(this))
            .build()

        dialogs = PlatformDialogs(this)
        bridge = NativeBridge(
            context = this,
            dialogs = dialogs,
            contextForApp = { appId -> sandboxContextFromManifest(appId) },
        )
        if (BuildConfig.DEBUG) {
            devControlPlane = AndroidDevControlPlane(
                context = this,
                bridge = bridge,
                requestedPort = intent.getIntExtra("terrane_control_port", 0),
            ).also { it.start() }
        } else {
            Log.i("TerranePlatform", "Android dev control plane is disabled in release builds")
        }

        webView = WebView(this)
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        webView.settings.javaScriptEnabled = true
        webView.settings.allowFileAccess = false
        webView.settings.allowFileAccessFromFileURLs = false
        webView.settings.allowUniversalAccessFromFileURLs = false
        webView.settings.allowContentAccess = false
        webView.settings.safeBrowsingEnabled = true
        smokeProbe?.install(webView, this)
        webView.webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                return assetLoader.shouldInterceptRequest(request.url)
            }

            override fun onPageFinished(view: WebView, url: String) {
                if (url == "https://appassets.androidplatform.net/runtime/index.html") {
                    smokeProbe?.runOnce(view, bridge, this@MainActivity)
                }
            }
        }

        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            Log.e("TerranePlatform", "Android WebMessageListener support is required for the runtime bridge")
            throw IllegalStateException("Android WebMessageListener support is required for Terrane runtime bridge")
        }
        WebViewCompat.addWebMessageListener(
            webView,
            "TerranePlatformBridge",
            setOf("https://appassets.androidplatform.net"),
        ) { _, message, sourceOrigin, isMainFrame, replyProxy ->
            bridge.handleEnvelope(message.data ?: "{}", isMainFrame, sourceOrigin.toString()) { response ->
                replyProxy.postMessage(response)
            }
        }

        setContentView(webView)
        webView.loadUrl("https://appassets.androidplatform.net/runtime/index.html")
    }

    override fun onDestroy() {
        devControlPlane?.stop()
        devControlPlane = null
        super.onDestroy()
    }

    private fun sandboxContextFromManifest(appId: String): AppSandboxContext {
        require(exampleAppIds.contains(appId)) { "Unknown generated app id" }
        val manifest = loadExampleManifest(appId)
        val actualAppId = manifest.optString("id", appId)
        return AppSandboxContext(
            appId = actualAppId,
            storagePrefix = manifest.optString("storagePrefix", "$actualAppId:"),
            approvedPermissions = manifest.optJSONArray("permissions").toStringSet { it },
            networkPolicy = NetworkPolicyRule.fromManifest(manifest),
            denyPrivateNetwork = manifest.optJSONObject("networkPolicy")?.optBoolean("denyPrivateNetwork", true) ?: true,
            resourceBudget = manifest.optJSONObject("resourceBudget") ?: JSONObject(),
        )
    }

    private fun loadExampleManifest(appId: String): JSONObject {
        val path = "webapps/examples/$appId/manifest.json"
        return assets.open(path).bufferedReader(Charsets.UTF_8).use { reader ->
            JSONObject(reader.readText())
        }
    }
}

private class AndroidSmokeProbe(
    private val action: AndroidSmokeAction,
    private val storageKey: String?,
    private val storageValue: String?,
    private val exitAfterSmoke: Boolean,
) {
    private var didRun = false

    fun install(webView: WebView, activity: MainActivity) {
        webView.webChromeClient = object : WebChromeClient() {
            override fun onConsoleMessage(consoleMessage: ConsoleMessage): Boolean {
                val message = consoleMessage.message()
                if (message.startsWith(failureMarker)) {
                    Log.e(tag, message)
                    if (exitAfterSmoke) {
                        webView.postDelayed({ activity.finishAndRemoveTask() }, 250)
                    }
                    return true
                }
                if (message.startsWith(successMarker)) {
                    Log.i(tag, message)
                    if (exitAfterSmoke) {
                        webView.postDelayed({ activity.finishAndRemoveTask() }, 250)
                    }
                    return true
                }
                return super.onConsoleMessage(consoleMessage)
            }
        }
    }

    fun runOnce(webView: WebView, bridge: NativeBridge, activity: MainActivity) {
        if (didRun) return
        didRun = true
        Log.i(tag, "ANDROID_SMOKE_STARTED_$action")
        if (action != AndroidSmokeAction.RuntimeLoad) {
            runBridgeSmoke(webView, bridge, activity)
            return
        }
        webView.evaluateJavascript(javaScript(), null)
    }

    private fun runBridgeSmoke(webView: WebView, bridge: NativeBridge, activity: MainActivity) {
        try {
            when (action) {
                AndroidSmokeAction.StorageSet -> {
                    val response = bridgeCall(
                        bridge = bridge,
                        appId = "notes-lite",
                        id = "android_smoke_storage_set",
                        method = "storage.set",
                        params = JSONObject(
                            mapOf(
                                "key" to (storageKey ?: ""),
                                "value" to JSONObject(mapOf("smokeValue" to (storageValue ?: ""))),
                            ),
                        ),
                    )
                    require(response.optBoolean("ok") && response.optJSONObject("result")?.optBoolean("ok") == true) {
                        "storage.set failed: $response"
                    }
                    emitSuccess(webView, activity, "TERRANE_ANDROID_SMOKE_STORAGE_SET_OK")
                }
                AndroidSmokeAction.StorageGet -> {
                    val response = bridgeCall(
                        bridge = bridge,
                        appId = "notes-lite",
                        id = "android_smoke_storage_get",
                        method = "storage.get",
                        params = JSONObject(mapOf("key" to (storageKey ?: ""), "defaultValue" to JSONObject.NULL)),
                    )
                    val actual = response.optJSONObject("result")
                        ?.optJSONObject("value")
                        ?.optString("smokeValue")
                    require(response.optBoolean("ok") && actual == storageValue) {
                        "storage.get mismatch: $response"
                    }
                    emitSuccess(webView, activity, "TERRANE_ANDROID_SMOKE_STORAGE_GET_OK")
                }
                AndroidSmokeAction.CoreStep -> {
                    val caps = bridgeCall(
                        bridge = bridge,
                        appId = "task-workbench",
                        id = "android_smoke_core_caps",
                        method = "runtime.capabilities",
                        params = JSONObject(),
                    )
                    require(caps.optJSONObject("result")
                        ?.optJSONObject("features")
                        ?.optBoolean("core.step") == true) {
                        "core.step is unavailable: $caps"
                    }
                    val response = bridgeCall(
                        bridge = bridge,
                        appId = "task-workbench",
                        id = "android_smoke_core_step",
                        method = "core.step",
                        params = JSONObject(
                            mapOf(
                                "event" to JSONObject(
                                    mapOf(
                                        "type" to "CreateTask",
                                        "payload" to JSONObject(mapOf("title" to "Android smoke task")),
                                    ),
                                ),
                            ),
                        ),
                    )
                    require(response.optBoolean("ok") &&
                        response.optJSONObject("result")?.optBoolean("ok") == true &&
                        response.optJSONObject("result")?.optJSONArray("actions") != null) {
                        "core.step failed: $response"
                    }
                    require(hasPersistedCoreLogs(activity, "task-workbench")) {
                        "core smoke did not persist bridge/core log rows"
                    }
                    emitSuccess(webView, activity, "TERRANE_ANDROID_SMOKE_CORE_STEP_OK")
                }
                AndroidSmokeAction.RuntimeLoad -> Unit
            }
        } catch (error: Exception) {
            Log.e(tag, "TERRANE_ANDROID_SMOKE_FAILED: ${error.message}", error)
            finishAfterSmoke(webView, activity)
        }
    }

    private fun bridgeCall(bridge: NativeBridge, appId: String, id: String, method: String, params: JSONObject): JSONObject {
        var responseText: String? = null
        val envelope = JSONObject(
            mapOf(
                "appId" to appId,
                "mountToken" to "android-smoke",
                "request" to JSONObject(mapOf("id" to id, "method" to method, "params" to params)),
            ),
        )
        bridge.handleEnvelope(
            body = envelope.toString(),
            isMainFrame = true,
            sourceOrigin = "https://appassets.androidplatform.net",
        ) { response ->
            responseText = response
        }
        return JSONObject(requireNotNull(responseText) { "Bridge did not respond to $method" })
    }

    private fun hasPersistedCoreLogs(context: Context, appId: String): Boolean {
        val database = PlatformDatabase(context)
        return try {
            val db = database.readableDatabase
            rowCount(db, "bridge_calls", appId, "core.step") > 0 &&
                rowCount(db, "core_events", appId) > 0 &&
                rowCount(db, "core_actions", appId) > 0
        } finally {
            database.close()
        }
    }

    private fun rowCount(db: android.database.sqlite.SQLiteDatabase, table: String, appId: String, method: String? = null): Int {
        val sql = if (method == null) {
            "SELECT COUNT(*) FROM $table WHERE app_id = ?"
        } else {
            "SELECT COUNT(*) FROM $table WHERE app_id = ? AND method = ?"
        }
        val args = if (method == null) arrayOf(appId) else arrayOf(appId, method)
        db.rawQuery(sql, args).use { cursor ->
            return if (cursor.moveToFirst()) cursor.getInt(0) else 0
        }
    }

    private fun emitSuccess(webView: WebView, activity: MainActivity, marker: String) {
        Log.i(tag, marker)
        finishAfterSmoke(webView, activity)
    }

    private fun finishAfterSmoke(webView: WebView, activity: MainActivity) {
        if (exitAfterSmoke) {
            webView.postDelayed({ activity.finishAndRemoveTask() }, 250)
        }
    }

    private fun javaScript(): String {
        val body = when (action) {
            AndroidSmokeAction.RuntimeLoad -> """
                await waitFor(function () {
                  return document.querySelector('[data-testid="runtime-status"]') &&
                    document.querySelector('[data-testid="runtime-status"]').textContent === "Ready";
                }, "runtime ready");
                console.log("TERRANE_ANDROID_SMOKE_RUNTIME_LOADED");
            """
            AndroidSmokeAction.StorageSet -> storageScript(
                marker = "TERRANE_ANDROID_SMOKE_STORAGE_SET_OK",
                operation = """
                    const setResponse = await nativeCall("notes-lite", "android_smoke_storage_set", "storage.set", { key: key, value: { smokeValue: value } });
                    if (!setResponse || !setResponse.ok || !setResponse.result || setResponse.result.ok !== true) {
                      throw new Error("storage.set failed: " + JSON.stringify(setResponse));
                    }
                """,
            )
            AndroidSmokeAction.StorageGet -> storageScript(
                marker = "TERRANE_ANDROID_SMOKE_STORAGE_GET_OK",
                operation = """
                    const getResponse = await nativeCall("notes-lite", "android_smoke_storage_get", "storage.get", { key: key, defaultValue: null });
                    const actual = getResponse && getResponse.result && getResponse.result.value && getResponse.result.value.smokeValue;
                    if (actual !== value) {
                      throw new Error("storage.get mismatch: " + JSON.stringify(getResponse));
                    }
                """,
            )
            AndroidSmokeAction.CoreStep -> """
                await waitForRuntimeReady();
                const capabilitiesResponse = await nativeCall("task-workbench", "android_smoke_core_caps", "runtime.capabilities", {});
                const capabilities = capabilitiesResponse && capabilitiesResponse.result;
                if (!capabilitiesResponse || !capabilitiesResponse.ok || !capabilities || capabilities.platform !== "android" || capabilities.features["core.step"] !== true) {
                  throw new Error("core.step is unavailable: " + JSON.stringify(capabilities));
                }
                const coreResponse = await nativeCall("task-workbench", "android_smoke_core_step", "core.step", {
                  event: { type: "CreateTask", payload: { title: "Android smoke task" } }
                });
                if (!coreResponse || !coreResponse.ok || !coreResponse.result || coreResponse.result.ok !== true || !Array.isArray(coreResponse.result.actions)) {
                  throw new Error("core.step failed: " + JSON.stringify(coreResponse));
                }
                console.log("TERRANE_ANDROID_SMOKE_CORE_STEP_OK");
            """
        }
        return """
            (async function () {
              try {
                async function waitFor(getter, label) {
                  const deadline = Date.now() + 15000;
                  while (Date.now() < deadline) {
                    const value = getter();
                    if (value) return value;
                    await new Promise(function (resolve) { setTimeout(resolve, 100); });
                  }
                  throw new Error("Timed out waiting for " + label);
                }
                async function waitForRuntimeReady() {
                  return await waitFor(function () {
                    return document.querySelector('[data-testid="runtime-status"]') &&
                      document.querySelector('[data-testid="runtime-status"]').textContent === "Ready";
                  }, "runtime ready");
                }
                const androidBridgePending = new Map();
                const androidBridge = await waitFor(function () {
                  return window.TerranePlatformBridge &&
                    typeof window.TerranePlatformBridge.postMessage === "function" &&
                    window.TerranePlatformBridge;
                }, "TerranePlatformBridge");
                androidBridge.onmessage = function (event) {
                  const response = typeof event.data === "string" ? JSON.parse(event.data) : event.data;
                  const responseId = response && response.id;
                  const waiter = responseId && androidBridgePending.get(responseId);
                  if (!waiter) return;
                  androidBridgePending.delete(responseId);
                  waiter.resolve(response);
                };
                function nativeCall(appId, id, method, params) {
                  return new Promise(function (resolve, reject) {
                    androidBridgePending.set(id, { resolve: resolve, reject: reject });
                    setTimeout(function () {
                      if (!androidBridgePending.has(id)) return;
                      androidBridgePending.delete(id);
                      reject(new Error("Timed out waiting for " + method));
                    }, 10000);
                    androidBridge.postMessage(JSON.stringify({
                      appId: appId,
                      mountToken: "android-smoke",
                      request: { id: id, method: method, params: params || {} }
                    }));
                  });
                }
                $body
              } catch (error) {
                console.error("TERRANE_ANDROID_SMOKE_FAILED: " + (error && error.message ? error.message : String(error)));
              }
            })();
        """
    }

    private fun storageScript(marker: String, operation: String): String {
        val key = JSONObject.quote(storageKey ?: "")
        val value = JSONObject.quote(storageValue ?: "")
        return """
            await waitForRuntimeReady();
            const capabilitiesResponse = await nativeCall("notes-lite", "android_smoke_storage_caps", "runtime.capabilities", {});
            const capabilities = capabilitiesResponse && capabilitiesResponse.result;
            if (!capabilitiesResponse || !capabilitiesResponse.ok || !capabilities || capabilities.platform !== "android" || capabilities.appId !== "notes-lite") {
              throw new Error("runtime.capabilities failed: " + JSON.stringify(capabilities));
            }
            const key = $key;
            const value = $value;
            $operation
            console.log("$marker");
        """
    }

    companion object {
        private const val tag = "TerranePlatformSmoke"
        private const val successMarker = "TERRANE_ANDROID_SMOKE_"
        private const val failureMarker = "TERRANE_ANDROID_SMOKE_FAILED"

        fun fromIntent(intent: Intent): AndroidSmokeProbe? {
            val storageAction = intent.getStringExtra("terrane_smoke_storage_action")
            val action = when {
                storageAction == "set" -> AndroidSmokeAction.StorageSet
                storageAction == "get" -> AndroidSmokeAction.StorageGet
                intent.getBooleanExtra("terrane_smoke_core_step", false) -> AndroidSmokeAction.CoreStep
                intent.getBooleanExtra("terrane_smoke_runtime_load", false) -> AndroidSmokeAction.RuntimeLoad
                else -> null
            } ?: return null
            return AndroidSmokeProbe(
                action = action,
                storageKey = intent.getStringExtra("terrane_smoke_storage_key"),
                storageValue = intent.getStringExtra("terrane_smoke_storage_value"),
                exitAfterSmoke = intent.getBooleanExtra("terrane_smoke_exit_after", false),
            )
        }
    }
}

private enum class AndroidSmokeAction {
    RuntimeLoad,
    StorageSet,
    StorageGet,
    CoreStep,
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
