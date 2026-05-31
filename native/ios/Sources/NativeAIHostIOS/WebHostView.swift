import SwiftUI
import SQLite3
import UIKit
import WebKit

#if DEBUG
import Darwin
#endif

struct WebHostView: UIViewRepresentable {
    func makeUIView(context: Context) -> WKWebView {
        let bridge = context.coordinator.bridge
        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(RuntimeSchemeHandler(), forURLScheme: RuntimeResourceLocator.scheme)

        let webView = WKWebView(frame: .zero, configuration: configuration)
        bridge.setDialogPresenterProvider { [weak webView] in
            guard let webView else { return nil }
            return Self.presentingViewController(from: webView)
        }
#if DEBUG
        if let smokeProbe = context.coordinator.smokeProbe {
            webView.navigationDelegate = smokeProbe
        }
#endif
        webView.load(URLRequest(url: RuntimeResourceLocator.runtimeIndexURL()))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    @MainActor
    final class Coordinator {
        let bridge = WebBridge()
#if DEBUG
        let smokeProbe = IOSSmokeRuntimeProbe.fromCommandLine()
#endif
    }

    @MainActor
    private static func presentingViewController(from view: UIView) -> UIViewController? {
        var responder: UIResponder? = view
        while let current = responder {
            if let viewController = current as? UIViewController {
                return viewController
            }
            responder = current.next
        }
        return view.window?.rootViewController
    }
}

#if DEBUG
final class IOSSmokeRuntimeProbe: NSObject, WKNavigationDelegate {
    static let loadedMarker = "NATIVE_AI_IOS_SMOKE_RUNTIME_LOADED"
    static let storageSetMarker = "NATIVE_AI_IOS_SMOKE_STORAGE_SET_OK"
    static let storageGetMarker = "NATIVE_AI_IOS_SMOKE_STORAGE_GET_OK"
    static let coreStepMarker = "NATIVE_AI_IOS_SMOKE_CORE_STEP_OK"
    static let markerFileName = "native-ai-ios-smoke-runtime-loaded.txt"

    private let exitAfterLoad: Bool
    private let storageSmoke: StorageSmoke?
    private let coreStepSmoke: Bool

    private init(exitAfterLoad: Bool, storageSmoke: StorageSmoke?, coreStepSmoke: Bool) {
        self.exitAfterLoad = exitAfterLoad
        self.storageSmoke = storageSmoke
        self.coreStepSmoke = coreStepSmoke
    }

    static func fromCommandLine() -> IOSSmokeRuntimeProbe? {
        let args = CommandLine.arguments
        let storageSmoke = StorageSmoke.fromCommandLine(args)
        let coreStepSmoke = args.contains("--native-ai-smoke-core-step")
        guard args.contains("--native-ai-smoke-runtime-load") ||
            args.contains("--native-ai-smoke-exit-on-runtime-load") ||
            storageSmoke != nil ||
            coreStepSmoke
        else {
            return nil
        }
        return IOSSmokeRuntimeProbe(
            exitAfterLoad: args.contains("--native-ai-smoke-exit-on-runtime-load"),
            storageSmoke: storageSmoke,
            coreStepSmoke: coreStepSmoke
        )
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard webView.url == RuntimeResourceLocator.runtimeIndexURL() else { return }
        emitSmokeMarker(Self.loadedMarker)
        if let storageSmoke {
            runStorageSmoke(storageSmoke, in: webView)
            return
        }
        if coreStepSmoke {
            runCoreStepSmoke(in: webView)
            return
        }
        exitIfRequested()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        emitSmokeMarker("NATIVE_AI_IOS_SMOKE_RUNTIME_FAILED: \(error.localizedDescription)")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        emitSmokeMarker("NATIVE_AI_IOS_SMOKE_RUNTIME_FAILED: \(error.localizedDescription)")
    }

    private func emitSmokeMarker(_ marker: String) {
        print(marker)
        fflush(stdout)
        let markerURL = FileManager.default.temporaryDirectory.appendingPathComponent(Self.markerFileName)
        try? marker.write(to: markerURL, atomically: true, encoding: .utf8)
    }

    private func runCoreStepSmoke(in webView: WKWebView) {
        runAsyncBridgeSmoke(
            script: CoreStepSmoke.javaScript(),
            successMarker: Self.coreStepMarker,
            in: webView
        )
    }

    private func runStorageSmoke(_ smoke: StorageSmoke, in webView: WKWebView) {
        runAsyncBridgeSmoke(
            script: smoke.javaScript(),
            successMarker: smoke.successMarker,
            in: webView
        )
    }

    private func runAsyncBridgeSmoke(script: String, successMarker: String, in webView: WKWebView) {
        Task { @MainActor [weak self, weak webView] in
            guard let webView else {
                self?.emitSmokeMarker("NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: web view released")
                self?.exitIfRequested()
                return
            }
            do {
                let value = try await webView.callAsyncJavaScript(script, arguments: [:], in: nil, contentWorld: .page)
                guard let marker = value as? String, marker == successMarker else {
                    self?.emitSmokeMarker("NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: unexpected result \(String(describing: value))")
                    self?.exitIfRequested()
                    return
                }
                if marker == Self.coreStepMarker && !Self.hasPersistedCoreLogs(appId: "task-workbench") {
                    self?.emitSmokeMarker("NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: core smoke did not persist bridge/core log rows")
                    self?.exitIfRequested()
                    return
                }
                self?.emitSmokeMarker(marker)
                self?.exitIfRequested()
            } catch {
                self?.emitSmokeMarker("NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: \(error.localizedDescription)")
                self?.exitIfRequested()
            }
        }
    }

    private static func hasPersistedCoreLogs(appId: String) -> Bool {
        let database = PlatformDatabase()
        guard let db = database.handle else { return false }
        return rowCount(db: db, table: "bridge_calls", appId: appId, method: "core.step") > 0 &&
            rowCount(db: db, table: "core_events", appId: appId) > 0 &&
            rowCount(db: db, table: "core_actions", appId: appId) > 0
    }

    private static func rowCount(db: OpaquePointer, table: String, appId: String, method: String? = nil) -> Int {
        let sql = method == nil
            ? "SELECT COUNT(*) FROM \(table) WHERE app_id = ?"
            : "SELECT COUNT(*) FROM \(table) WHERE app_id = ? AND method = ?"
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &statement, nil) == SQLITE_OK else {
            return 0
        }
        defer { sqlite3_finalize(statement) }
        sqlite3_bind_text(statement, 1, appId, -1, SQLITE_TRANSIENT)
        if let method {
            sqlite3_bind_text(statement, 2, method, -1, SQLITE_TRANSIENT)
        }
        return sqlite3_step(statement) == SQLITE_ROW ? Int(sqlite3_column_int(statement, 0)) : 0
    }

    private func exitIfRequested() {
        guard exitAfterLoad else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            Darwin.exit(0)
        }
    }
}

private let SQLITE_TRANSIENT = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

private struct CoreStepSmoke {
    static func javaScript() -> String {
        """
        try {
          const bridge = window.webkit &&
            window.webkit.messageHandlers &&
            window.webkit.messageHandlers.NativeAIPlatformBridge;
          if (!bridge || typeof bridge.postMessage !== "function") {
            throw new Error("NativeAIPlatformBridge is unavailable");
          }
          const appId = "task-workbench";
          const mountToken = "ios-smoke";
          function request(id, method, params) {
            return { appId, mountToken, request: { id, method, params: params || {} } };
          }
          const capabilities = await bridge.postMessage(request("ios_smoke_capabilities", "runtime.capabilities", {}));
          if (!capabilities || !capabilities.ok || !capabilities.result || capabilities.result.features["core.step"] !== true) {
            throw new Error("core.step is not available: " + JSON.stringify(capabilities));
          }
          const response = await bridge.postMessage(request("ios_smoke_core_step", "core.step", {
            event: { type: "CreateTask", payload: { title: "iOS smoke task" } }
          }));
          if (!response || !response.ok || !response.result || response.result.ok !== true || !Array.isArray(response.result.actions)) {
            throw new Error("core.step failed: " + JSON.stringify(response));
          }
          return "NATIVE_AI_IOS_SMOKE_CORE_STEP_OK";
        } catch (error) {
          return "NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: " + (error && error.message ? error.message : String(error));
        }
        """
    }
}

private struct StorageSmoke {
    enum Action {
        case set
        case get
    }

    let action: Action
    let key: String
    let value: String

    var successMarker: String {
        switch action {
        case .set:
            return "NATIVE_AI_IOS_SMOKE_STORAGE_SET_OK"
        case .get:
            return "NATIVE_AI_IOS_SMOKE_STORAGE_GET_OK"
        }
    }

    static func fromCommandLine(_ args: [String]) -> StorageSmoke? {
        let action: Action?
        if args.contains("--native-ai-smoke-storage-set") {
            action = .set
        } else if args.contains("--native-ai-smoke-storage-get") {
            action = .get
        } else {
            action = nil
        }
        guard let action,
              let key = value(after: "--native-ai-smoke-storage-key", in: args),
              let value = value(after: "--native-ai-smoke-storage-value", in: args)
        else {
            return nil
        }
        return StorageSmoke(action: action, key: key, value: value)
    }

    func javaScript() -> String {
        let keyLiteral = javaScriptStringLiteral(key)
        let valueLiteral = javaScriptStringLiteral(value)
        let markerLiteral = javaScriptStringLiteral(successMarker)
        let actionScript: String
        switch action {
        case .set:
            actionScript = """
            const setResponse = await bridge.postMessage(request("ios_smoke_storage_set", "storage.set", { key: key, value: { smokeValue: value } }));
            if (!setResponse || !setResponse.ok) {
              throw new Error("storage.set failed: " + JSON.stringify(setResponse));
            }
            return marker;
            """
        case .get:
            actionScript = """
            const getResponse = await bridge.postMessage(request("ios_smoke_storage_get", "storage.get", { key: key }));
            const actual = getResponse && getResponse.result && getResponse.result.value && getResponse.result.value.smokeValue;
            if (!getResponse || !getResponse.ok || actual !== value) {
              throw new Error("storage.get mismatch: " + JSON.stringify(getResponse));
            }
            return marker;
            """
        }
        return """
        try {
          const bridge = window.webkit &&
            window.webkit.messageHandlers &&
            window.webkit.messageHandlers.NativeAIPlatformBridge;
          if (!bridge || typeof bridge.postMessage !== "function") {
            throw new Error("NativeAIPlatformBridge is unavailable");
          }
          const appId = "notes-lite";
          const mountToken = "ios-smoke";
          const key = \(keyLiteral);
          const value = \(valueLiteral);
          const marker = \(markerLiteral);
          function request(id, method, params) {
            return { appId, mountToken, request: { id, method, params: params || {} } };
          }
          const capabilities = await bridge.postMessage(request("ios_smoke_capabilities", "runtime.capabilities", {}));
          if (!capabilities || !capabilities.ok || !capabilities.result || capabilities.result.platform !== "ios") {
            throw new Error("runtime.capabilities failed: " + JSON.stringify(capabilities));
          }
        \(actionScript)
        } catch (error) {
          return "NATIVE_AI_IOS_SMOKE_BRIDGE_FAILED: " + (error && error.message ? error.message : String(error));
        }
        """
    }

    private static func value(after name: String, in args: [String]) -> String? {
        guard let index = args.firstIndex(of: name),
              args.indices.contains(args.index(after: index))
        else {
            return nil
        }
        return args[args.index(after: index)]
    }

    private func javaScriptStringLiteral(_ value: String) -> String {
        guard let data = try? JSONEncoder().encode(value),
              let encoded = String(data: data, encoding: .utf8)
        else {
            return "\"\""
        }
        return encoded
    }
}
#endif

private let appRuntimeUserScript = """
(function(){
if(window.AppRuntime)return;
if(window.location.protocol!=="app-runtime:"||window.location.hostname==="runtime")return;
var runtimeAppId=window.location.hostname;
var knownEvents=new Set(["runtime.ready","runtime.suspend","runtime.resume","app.error","app.budget_warning","app.permission_revoked"]);
var eventHandlers=new Map();
var nextId=1;
var port=null;
var pending=new Map();
var queued=[];
function emit(eventName,payload){
var handlers=eventHandlers.get(eventName);
if(!handlers||!handlers.size)return;
Array.from(handlers).forEach(function(handler){try{handler(payload||{});}catch(error){console.error("AppRuntime event handler failed",error);}});
}
function emitAppError(error,source){
emit("app.error",{code:error&&error.code?error.code:"runtime_error",message:error&&error.message?error.message:String(error||"Unknown runtime error"),source:source});
}
function send(message){port.postMessage(message);}
function call(method,params){
return new Promise(function(resolve,reject){
if(typeof method!=="string"||!method){reject({code:"invalid_request",message:"Bridge method must be a non-empty string",details:{}});return;}
var bodyParams=params==null?{}:params;
if(typeof bodyParams!=="object"||Array.isArray(bodyParams)){reject({code:"invalid_request",message:"Bridge params must be an object",details:{}});return;}
var id="app_req_"+nextId++;
var message={id:id,method:method,params:bodyParams,timestamp:Date.now()};
pending.set(id,{resolve:resolve,reject:reject});
if(port)send(message);else queued.push(message);
});
}
function on(eventName,handler){
if(!knownEvents.has(eventName)||typeof handler!=="function")return function(){};
if(!eventHandlers.has(eventName))eventHandlers.set(eventName,new Set());
var handlers=eventHandlers.get(eventName);handlers.add(handler);
return function(){handlers.delete(handler);};
}
window.AppRuntime={call:call,capabilities:function(){return call("runtime.capabilities",{});},on:on};
window.addEventListener("message",function(event){
if(!event.data||event.data.type!=="runtime.port"||!event.ports||!event.ports[0])return;
port=event.ports[0];
port.onmessage=function(portEvent){
var response=portEvent.data;
if(response&&response.type==="runtime.event"){emit(response.eventName,response.payload||{});return;}
var waiter=pending.get(response.id);
if(!waiter)return;
pending.delete(response.id);
if(response.ok)waiter.resolve(response.result);
else{emitAppError(response.error,"bridge");waiter.reject(response.error);}
};
while(queued.length)send(queued.shift());
call("runtime.capabilities",{}).then(function(capabilities){emit("runtime.ready",{runtimeVersion:capabilities.runtimeVersion||"0.1.0",appId:runtimeAppId,capabilities:capabilities});}).catch(function(error){emitAppError(error,"runtime.ready");});
});
window.parent.postMessage({type:"runtime.ready_for_port"},"*");
})();
"""

final class RuntimeSchemeHandler: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        if let requestURL = urlSchemeTask.request.url,
           RuntimeResourceLocator.isBundledAppIndexURL(requestURL) {
            let data = BundledAppCatalog.appIndexData()
            let response = HTTPURLResponse(
                url: requestURL,
                statusCode: 200,
                httpVersion: nil,
                headerFields: [
                    "Content-Type": "application/json; charset=utf-8",
                    "Content-Length": "\(data.count)"
                ]
            )!
            urlSchemeTask.didReceive(response)
            urlSchemeTask.didReceive(data)
            urlSchemeTask.didFinish()
            return
        }

        guard let requestURL = urlSchemeTask.request.url,
              let fileURL = RuntimeResourceLocator.fileURL(forRuntimeURL: requestURL)
        else {
            urlSchemeTask.didFailWithError(RuntimeResourceError.notFound)
            return
        }

        do {
            let data = try Self.data(for: fileURL, requestURL: requestURL)
            let response = HTTPURLResponse(
                url: requestURL,
                statusCode: 200,
                httpVersion: nil,
                headerFields: [
                    "Content-Type": "\(RuntimeResourceLocator.mimeType(for: fileURL)); charset=utf-8",
                    "Content-Length": "\(data.count)"
                ]
            )!
            urlSchemeTask.didReceive(response)
            urlSchemeTask.didReceive(data)
            urlSchemeTask.didFinish()
        } catch {
            urlSchemeTask.didFailWithError(error)
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {}

    private static func data(for fileURL: URL, requestURL: URL) throws -> Data {
        let data = try Data(contentsOf: fileURL)
        guard RuntimeResourceLocator.isGeneratedAppIndexURL(requestURL),
              let html = String(data: data, encoding: .utf8),
              let transformed = htmlWithAppRuntimeBootstrap(html).data(using: .utf8)
        else {
            return data
        }
        return transformed
    }

    private static func htmlWithAppRuntimeBootstrap(_ html: String) -> String {
        let cspAdjusted = htmlWithAppRuntimeCSP(html)
        let bootstrap = "<script>\(appRuntimeUserScript)</script>"
        guard let head = cspAdjusted.range(of: "<head>") else {
            return bootstrap + cspAdjusted
        }
        return String(cspAdjusted[..<head.upperBound]) + bootstrap + String(cspAdjusted[head.upperBound...])
    }

    private static func htmlWithAppRuntimeCSP(_ html: String) -> String {
        replaceFirst(
            replaceFirst(
                replaceFirst(
                    replaceFirst(html, "script-src 'self';", "script-src 'self' app-runtime:;"),
                    "style-src 'self';",
                    "style-src 'self' app-runtime:;"
                ),
                "img-src 'self' data: blob:;",
                "img-src 'self' app-runtime: data: blob:;"
            ),
            "font-src 'self';",
            "font-src 'self' app-runtime:;"
        )
    }

    private static func replaceFirst(_ text: String, _ needle: String, _ replacement: String) -> String {
        guard let range = text.range(of: needle) else {
            return text
        }
        return text.replacingCharacters(in: range, with: replacement)
    }
}

enum RuntimeResourceLocator {
    static let scheme = "app-runtime"

    static func runtimeIndexURL() -> URL {
        URL(string: "\(scheme)://runtime/index.html")!
    }

    static func isBundledAppIndexURL(_ url: URL) -> Bool {
        url.scheme == scheme && logicalResourcePath(for: url) == "runtime/app-index.json"
    }

    static func isGeneratedAppIndexURL(_ url: URL) -> Bool {
        let logicalPath = logicalResourcePath(for: url)
        return logicalPath.hasPrefix("webapps/examples/") && logicalPath.hasSuffix("/index.html")
    }

    static func exampleManifestURL(for appId: String) -> URL? {
        if let bundled = Bundle.main.resourceURL?
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent("manifest.json"),
            FileManager.default.fileExists(atPath: bundled.path) {
            return bundled
        }
        if let bundled = Bundle.main.url(forResource: "manifest", withExtension: "json", subdirectory: "examples/\(appId)") {
            return bundled
        }
        return repoRootURL()
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent("manifest.json")
    }

    static func fileURL(forRuntimeURL url: URL) -> URL? {
        guard url.scheme == scheme else { return nil }
        let logicalPath = logicalResourcePath(for: url)
        guard isAllowedLogicalPath(logicalPath) else { return nil }

        if logicalPath.hasPrefix("runtime/") {
            let relative = String(logicalPath.dropFirst("runtime/".count))
            return firstExistingURL([
                Bundle.main.resourceURL?.appendingPathComponent("runtime").appendingPathComponent(relative),
                repoRootURL().appendingPathComponent("runtime-web").appendingPathComponent(relative)
            ])
        }

        if logicalPath.hasPrefix("webapps/examples/") {
            return firstExistingURL([
                Bundle.main.resourceURL?.appendingPathComponent(logicalPath),
                repoRootURL().appendingPathComponent(logicalPath)
            ])
        }

        return nil
    }

    static func mimeType(for fileURL: URL) -> String {
        switch fileURL.pathExtension.lowercased() {
        case "html":
            return "text/html"
        case "css":
            return "text/css"
        case "js":
            return "text/javascript"
        case "json":
            return "application/json"
        default:
            return "text/plain"
        }
    }

    static func repoRootURL() -> URL {
        var url = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<4 {
            if FileManager.default.fileExists(atPath: url.appendingPathComponent("docs/00_PRD.md").path) {
                return url
            }
            url.deleteLastPathComponent()
        }
        return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    }

    private static func logicalResourcePath(for url: URL) -> String {
        let path = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if url.host == "runtime", path == "index.html" {
            return "runtime/index.html"
        }
        if let host = url.host, host != "runtime" {
            return "webapps/examples/\(host)/\(path.isEmpty ? "index.html" : path)"
        }
        return path
    }

    private static func isAllowedLogicalPath(_ path: String) -> Bool {
        !path.isEmpty &&
            !path.contains("..") &&
            !path.contains("\\") &&
            (path.hasPrefix("runtime/") || path.hasPrefix("webapps/examples/"))
    }

    private static func firstExistingURL(_ urls: [URL?]) -> URL? {
        urls.compactMap { $0 }.first { FileManager.default.fileExists(atPath: $0.path) }
    }
}

enum RuntimeResourceError {
    static let notFound = NSError(
        domain: NSURLErrorDomain,
        code: NSURLErrorFileDoesNotExist,
        userInfo: [NSLocalizedDescriptionKey: "Runtime resource was not found"]
    )
}
