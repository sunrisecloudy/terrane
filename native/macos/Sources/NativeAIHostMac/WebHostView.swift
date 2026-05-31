import AppKit
import WebKit

final class WebHostView: NSView, WKNavigationDelegate {
    private let webView: WKWebView
    private let bridge: WebBridge
    private let crashRecovery: RuntimeCrashRecovery
    private let crashBanner = NSView(frame: .zero)
    private let crashLabel = NSTextField(labelWithString: "Runtime was interrupted")
    private let reloadButton = NSButton(title: "Reload", target: nil, action: nil)
    private var runtimeSessionId = RuntimeCrashRecovery.newSessionId()
    private var runtimeReady = false

    override init(frame frameRect: NSRect) {
        let bridge = WebBridge()
        self.bridge = bridge
        self.crashRecovery = RuntimeCrashRecovery()

        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(RuntimeSchemeHandler(), forURLScheme: RuntimeResourceLocator.scheme)

        self.webView = WKWebView(frame: .zero, configuration: configuration)
        super.init(frame: frameRect)

        webView.navigationDelegate = self
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
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
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

    static func isGeneratedAppIndexURL(_ url: URL) -> Bool {
        let logicalPath = logicalResourcePath(for: url)
        return logicalPath.hasPrefix("webapps/examples/") && logicalPath.hasSuffix("/index.html")
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
