import AppKit
import WebKit

final class WebHostView: NSView, WKNavigationDelegate {
    private let webView: WKWebView
    private let bridge: WebBridge
    private let nativeShellMessageHandler: NativeShellMessageHandler
    private let crashRecovery: RuntimeCrashRecovery
    private let crashBanner = NSView(frame: .zero)
    private let crashLabel = NSTextField(labelWithString: "Runtime was interrupted")
    private let reloadButton = NSButton(title: "Reload", target: nil, action: nil)
    private var runtimeSessionId = RuntimeCrashRecovery.newSessionId()
    private var runtimeReady = false
    private var nativeHostModeEnabled: Bool
    private var pendingNativeAppId: String?
    private var pendingMarketplaceView = false
    private var pendingEngineRoomView = false
    var onNativeRuntimeError: ((String) -> Void)?
    var onNativeAppMounted: ((String) -> Void)?

    init(frame frameRect: NSRect = .zero, nativeHostModeEnabled: Bool = true) {
        let bridge = WebBridge()
        self.bridge = bridge
        self.nativeShellMessageHandler = NativeShellMessageHandler()
        self.crashRecovery = RuntimeCrashRecovery()
        self.nativeHostModeEnabled = nativeHostModeEnabled

        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "TerranePlatformBridge")
        contentController.add(nativeShellMessageHandler, name: "TerraneNativeShell")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(RuntimeSchemeHandler(), forURLScheme: RuntimeResourceLocator.scheme)

        self.webView = WKWebView(frame: .zero, configuration: configuration)
        super.init(frame: frameRect)

        webView.navigationDelegate = self
        nativeShellMessageHandler.onActiveAppChanged = { [weak self] appId in
            self?.onNativeAppMounted?(appId)
        }
        addSubview(webView)
        configureCrashBanner()
        loadRuntime()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func layout() {
        super.layout()
        webView.frame = bounds
        let bannerHeight: CGFloat = 52
        crashBanner.frame = NSRect(x: 0, y: max(0, bounds.height - bannerHeight), width: bounds.width, height: bannerHeight)
        reloadButton.sizeToFit()
        let buttonSize = reloadButton.frame.size
        reloadButton.frame = NSRect(
            x: max(12, bounds.width - buttonSize.width - 16),
            y: (bannerHeight - buttonSize.height) / 2,
            width: buttonSize.width,
            height: buttonSize.height
        )
        crashLabel.frame = NSRect(
            x: 16,
            y: 0,
            width: max(0, reloadButton.frame.minX - 28),
            height: bannerHeight
        )
    }

    private func loadRuntime() {
        crashRecovery.startRuntimeSession(sessionId: runtimeSessionId)
        webView.load(URLRequest(url: RuntimeResourceLocator.runtimeIndexURL()))
    }

    func mountApp(id appId: String) {
        pendingNativeAppId = appId
        pendingMarketplaceView = false
        pendingEngineRoomView = false
        applyNativeRuntimeState()
    }

    func showMarketplace() {
        pendingNativeAppId = nil
        pendingMarketplaceView = true
        pendingEngineRoomView = false
        applyNativeRuntimeState()
    }

    func showEngineRoom() {
        pendingNativeAppId = nil
        pendingMarketplaceView = false
        pendingEngineRoomView = true
        applyNativeRuntimeState()
    }

    func setNativeHostModeEnabled(_ enabled: Bool) {
        nativeHostModeEnabled = enabled
        applyNativeRuntimeState()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        runtimeReady = true
        applyNativeRuntimeState()
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        let crash = crashRecovery.recordWebContentProcessTerminated(
            sessionId: runtimeSessionId,
            previousMountCompletedReady: runtimeReady
        )
        showCrashBanner(canAutoRemount: crash.canAutoRemount)
    }

    private func configureCrashBanner() {
        crashBanner.wantsLayer = true
        crashBanner.layer?.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.96).cgColor
        crashBanner.layer?.borderColor = NSColor.separatorColor.cgColor
        crashBanner.layer?.borderWidth = 1
        crashBanner.isHidden = true

        crashLabel.font = .systemFont(ofSize: 13, weight: .medium)
        crashLabel.textColor = .labelColor
        crashLabel.lineBreakMode = .byTruncatingTail
        crashLabel.alignment = .left
        crashLabel.cell?.wraps = false

        reloadButton.bezelStyle = .rounded
        reloadButton.target = self
        reloadButton.action = #selector(reloadAfterCrash)

        crashBanner.addSubview(crashLabel)
        crashBanner.addSubview(reloadButton)
        addSubview(crashBanner, positioned: .above, relativeTo: webView)
    }

    private func showCrashBanner(canAutoRemount: Bool) {
        crashLabel.stringValue = canAutoRemount
            ? "Runtime was interrupted after it became ready"
            : "Runtime was interrupted before it became ready"
        crashBanner.isHidden = false
        needsLayout = true
    }

    @objc private func reloadAfterCrash() {
        crashBanner.isHidden = true
        runtimeReady = false
        runtimeSessionId = RuntimeCrashRecovery.newSessionId()
        loadRuntime()
    }

    private func applyNativeRuntimeState() {
        guard runtimeReady else { return }
        let script = Self.nativeRuntimeUpdateScript(
            appId: pendingNativeAppId,
            showMarketplace: pendingMarketplaceView,
            showEngineRoom: pendingEngineRoomView,
            nativeHostModeEnabled: nativeHostModeEnabled
        )
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await webView.callAsyncJavaScript(
                    script,
                    arguments: [:],
                    in: nil,
                    contentWorld: .page
                )
            } catch {
                let message = "Terrane native runtime host update failed: \(Self.describeRuntimeError(error))"
                fputs("\(message)\n", stderr)
                onNativeRuntimeError?(message)
            }
        }
    }

    /// Describes a failed runtime update in a way that is actually actionable.
    ///
    /// WebKit reports every JavaScript exception with the same opaque
    /// `localizedDescription` ("A JavaScript exception occurred"); the thrown
    /// message and its source location live in the error's `userInfo` instead.
    /// Surfacing those turns an undiagnosable banner — which hides whatever the
    /// runtime/Engine Room actually threw — into one that names the real cause.
    /// Non-JavaScript errors (navigation, encoding) keep their normal description.
    static func describeRuntimeError(_ error: Error) -> String {
        let nsError = error as NSError
        guard nsError.domain == WKError.errorDomain,
              let rawMessage = nsError.userInfo[jsExceptionMessageKey] as? String,
              !rawMessage.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return nsError.localizedDescription
        }

        var description = rawMessage.trimmingCharacters(in: .whitespacesAndNewlines)
        // Line/column are zero when WebKit cannot attribute the exception (e.g. a
        // throw from an injected top-level script), so only append a location when
        // it points somewhere meaningful.
        let line = (nsError.userInfo[jsExceptionLineKey] as? NSNumber)?.intValue ?? 0
        if line > 0, let source = jsExceptionSourceName(from: nsError.userInfo[jsExceptionSourceKey]) {
            description += " (\(source):\(line))"
        }
        return description
    }

    private static func jsExceptionSourceName(from value: Any?) -> String? {
        if let url = value as? URL { return url.lastPathComponent }
        if let string = value as? String, !string.isEmpty {
            return URL(string: string)?.lastPathComponent ?? string
        }
        return nil
    }

    // WebKit populates these (non-public but stable) userInfo keys on a
    // WKError.javaScriptExceptionOccurred from callAsyncJavaScript/evaluateJavaScript.
    private static let jsExceptionMessageKey = "WKJavaScriptExceptionMessage"
    private static let jsExceptionLineKey = "WKJavaScriptExceptionLineNumber"
    private static let jsExceptionSourceKey = "WKJavaScriptExceptionSourceURL"

    static func nativeRuntimeUpdateScript(appId: String?, showMarketplace: Bool = false, showEngineRoom: Bool = false, nativeHostModeEnabled: Bool) -> String {
        var statements = [
            "const host = window.TerraneRuntimeHost;",
            "if (!host) { throw new Error('TerraneRuntimeHost is unavailable'); }",
            "host.setHostMode(\(nativeHostModeEnabled ? "true" : "false"));"
        ]
        if showEngineRoom {
            statements.append("await host.showEngineRoom();")
        } else if showMarketplace {
            statements.append("await host.showMarketplace();")
        } else if let appId {
            statements.append("await host.mountApp(\(Self.javascriptStringLiteral(appId)));")
        }
        statements.append("return { ok: true };")
        return statements.joined(separator: "\n")
    }

    private static func javascriptStringLiteral(_ value: String) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: [value]),
              let arrayLiteral = String(data: data, encoding: .utf8),
              arrayLiteral.first == "[",
              arrayLiteral.last == "]"
        else {
            return "\"\""
        }
        return String(arrayLiteral.dropFirst().dropLast())
    }
}

@MainActor
private final class NativeShellMessageHandler: NSObject, WKScriptMessageHandler {
    var onActiveAppChanged: ((String) -> Void)?

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              let body = message.body as? [String: Any],
              body["type"] as? String == "active_app_changed",
              let appId = body["appId"] as? String,
              !appId.isEmpty,
              appId.count <= 128
        else {
            return
        }
        onActiveAppChanged?(appId)
    }
}

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
    private let engineRoom = NativeEngineRoomSnapshotProvider()

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard let requestURL = urlSchemeTask.request.url else {
            urlSchemeTask.didFailWithError(RuntimeResourceError.notFound)
            return
        }

        if RuntimeResourceLocator.isEngineRoomSnapshotURL(requestURL) {
            sendEngineRoomSnapshot(urlSchemeTask, requestURL: requestURL)
            return
        }

        guard let fileURL = RuntimeResourceLocator.fileURL(forRuntimeURL: requestURL) else {
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

    private func sendEngineRoomSnapshot(_ urlSchemeTask: WKURLSchemeTask, requestURL: URL) {
        let requestBody = urlSchemeTask.request.httpBody.flatMap {
            try? JSONSerialization.jsonObject(with: $0) as? [String: Any]
        } ?? [:]
        let snapshot = engineRoom.snapshot(
            appId: requestBody["appId"] as? String,
            limit: requestBody["limit"] as? Int
        )
        let envelope: [String: Any] = ["ok": true, "result": snapshot]

        guard JSONSerialization.isValidJSONObject(envelope),
              let data = try? JSONSerialization.data(withJSONObject: envelope, options: [.sortedKeys])
        else {
            urlSchemeTask.didFailWithError(RuntimeResourceError.jsonEncodingFailed)
            return
        }

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
    }

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

/// Maps an app id to the directory that actually holds its files. Required because a
/// Premium app's id (e.g. `hold-dear-admin`) can differ from its on-disk path
/// (`hold-dear/admin`), so the `<dir>/<id>` convention alone cannot locate it. The
/// catalog populates this during its scan; resource resolution consults it first and
/// falls back to the convention for flat apps that were never registered.
final class AppDirectoryRegistry: @unchecked Sendable {
    static let shared = AppDirectoryRegistry()

    private var directoriesById: [String: URL] = [:]
    private let lock = NSLock()

    func register(appId: String, directory: URL) {
        lock.lock()
        defer { lock.unlock() }
        directoriesById[appId] = directory
    }

    func directory(for appId: String) -> URL? {
        lock.lock()
        defer { lock.unlock() }
        return directoriesById[appId]
    }
}

enum RuntimeResourceLocator {
    static let scheme = "app-runtime"

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

    static func runtimeIndexURL() -> URL {
        URL(string: "\(scheme)://runtime/index.html")!
    }

    static func runtimeDirectoryURL() -> URL? {
        firstExistingDirectory([
            Bundle.main.resourceURL?.appendingPathComponent("runtime"),
            repoRootURL().appendingPathComponent("runtime-web")
        ])
    }

    static func examplesDirectoryURL() -> URL? {
        firstExistingDirectory([
            Bundle.main.resourceURL?.appendingPathComponent("webapps/examples"),
            repoRootURL().appendingPathComponent("webapps/examples")
        ])
    }

    /// Optional directory of Premium apps, supplied by a dev/host seam rather than a
    /// dependency on the private Premium repo. Resolved from `TERRANE_PREMIUM_APPS_DIR`
    /// first (dev side-by-side checkout), then a `webapps/premium` bundle resource for
    /// packaged hosts. Returns nil when no Premium apps are available, leaving the host
    /// fully functional with only the bundled (free) catalog.
    static func premiumAppsDirectoryURL() -> URL? {
        firstExistingDirectory([
            premiumAppsOverrideURL(),
            Bundle.main.resourceURL?.appendingPathComponent("webapps/premium")
        ])
    }

    private static func premiumAppsOverrideURL() -> URL? {
        guard let path = ProcessInfo.processInfo.environment["TERRANE_PREMIUM_APPS_DIR"],
              !path.trimmingCharacters(in: .whitespaces).isEmpty
        else {
            return nil
        }
        return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
    }

    /// Directories searched, in order, when resolving a generated app by id or path.
    /// Bundled examples take precedence; Premium apps fill in when present.
    static func appSearchDirectories() -> [URL] {
        [examplesDirectoryURL(), premiumAppsDirectoryURL()].compactMap { $0 }
    }

    static func sqliteMigrationsDirectoryURL() -> URL? {
        firstExistingDirectory([
            Bundle.main.resourceURL?.appendingPathComponent("db/sqlite"),
            repoRootURL().appendingPathComponent("db/sqlite")
        ])
    }

    static func isGeneratedAppIndexURL(_ url: URL) -> Bool {
        let logicalPath = logicalResourcePath(for: url)
        return logicalPath.hasPrefix("webapps/examples/") && logicalPath.hasSuffix("/index.html")
    }

    static func isEngineRoomSnapshotURL(_ url: URL) -> Bool {
        url.scheme == scheme && url.host == "runtime" && logicalResourcePath(for: url) == "engine-room/snapshot"
    }

    static func exampleAppDirectoryURL(for appId: String) -> URL? {
        guard isSafePathSegment(appId) else { return nil }
        if let registered = AppDirectoryRegistry.shared.directory(for: appId),
           directoryExists(at: registered) {
            return registered
        }
        for base in appSearchDirectories() {
            let appDirectory = base.appendingPathComponent(appId)
            if directoryExists(at: appDirectory) { return appDirectory }
        }
        return nil
    }

    static func exampleManifestURL(for appId: String) -> URL? {
        guard let appDirectory = exampleAppDirectoryURL(for: appId) else { return nil }
        let manifestURL = appDirectory.appendingPathComponent("manifest.json")
        return FileManager.default.fileExists(atPath: manifestURL.path) ? manifestURL : nil
    }

    static func exampleFileURL(appId: String, path: String) -> URL? {
        guard isSafePathSegment(appId), isAllowedExampleRelativePath(path) else { return nil }
        guard let appDirectory = exampleAppDirectoryURL(for: appId) else { return nil }
        let fileURL = appDirectory.appendingPathComponent(path)
        return FileManager.default.fileExists(atPath: fileURL.path) ? fileURL : nil
    }

    static func fileURL(forRuntimeURL url: URL) -> URL? {
        guard url.scheme == scheme else { return nil }
        let logicalPath = logicalResourcePath(for: url)
        guard isAllowedLogicalPath(logicalPath) else { return nil }

        if logicalPath.hasPrefix("runtime/") {
            let relative = String(logicalPath.dropFirst("runtime/".count))
            guard let runtimeDirectory = runtimeDirectoryURL() else { return nil }
            let fileURL = runtimeDirectory.appendingPathComponent(relative)
            return FileManager.default.fileExists(atPath: fileURL.path) ? fileURL : nil
        }

        if logicalPath.hasPrefix("webapps/examples/") {
            let relative = String(logicalPath.dropFirst("webapps/examples/".count))
            // `relative` is "<appId>/<path...>"; resolve the app directory by id (which
            // honors the registry for nested apps) and append the in-app path.
            let parts = relative.split(separator: "/", maxSplits: 1, omittingEmptySubsequences: false)
            guard let first = parts.first, !first.isEmpty else { return nil }
            let appId = String(first)
            let subPath = parts.count > 1 ? String(parts[1]) : ""
            guard let appDirectory = exampleAppDirectoryURL(for: appId) else { return nil }
            let fileURL = subPath.isEmpty ? appDirectory : appDirectory.appendingPathComponent(subPath)
            return FileManager.default.fileExists(atPath: fileURL.path) ? fileURL : nil
        }

        return nil
    }

    static func mimeType(for fileURL: URL) -> String {
        ForgeDataCatalog.shared.mimeTypes.mimeType(for: fileURL.pathExtension)
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

    private static func isAllowedExampleRelativePath(_ path: String) -> Bool {
        !path.isEmpty &&
            !path.hasPrefix("/") &&
            !path.contains("..") &&
            !path.contains("\\")
    }

    private static func isSafePathSegment(_ value: String) -> Bool {
        !value.isEmpty &&
            !value.contains("..") &&
            !value.contains("/") &&
            !value.contains("\\")
    }

    private static func firstExistingDirectory(_ urls: [URL?]) -> URL? {
        urls.compactMap { $0 }.first { directoryExists(at: $0) }
    }

    private static func directoryExists(at url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }
}

enum RuntimeResourceError {
    static let notFound = NSError(
        domain: NSURLErrorDomain,
        code: NSURLErrorFileDoesNotExist,
        userInfo: [NSLocalizedDescriptionKey: "Runtime resource was not found"]
    )

    static let jsonEncodingFailed = NSError(
        domain: NSURLErrorDomain,
        code: NSURLErrorCannotParseResponse,
        userInfo: [NSLocalizedDescriptionKey: "Engine Room snapshot could not be encoded"]
    )
}
