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

final class RuntimeSchemeHandler: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard let requestURL = urlSchemeTask.request.url,
              let fileURL = RuntimeResourceLocator.fileURL(forRuntimeURL: requestURL)
        else {
            urlSchemeTask.didFailWithError(RuntimeResourceError.notFound)
            return
        }

        do {
            let data = try Data(contentsOf: fileURL)
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
