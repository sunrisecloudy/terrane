import AppKit
import WebKit

final class WebHostView: NSView {
    private let webView: WKWebView
    private let bridge: WebBridge

    override init(frame frameRect: NSRect) {
        let bridge = WebBridge()
        self.bridge = bridge

        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()

        self.webView = WKWebView(frame: .zero, configuration: configuration)
        super.init(frame: frameRect)

        addSubview(webView)
        loadRuntime()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func layout() {
        super.layout()
        webView.frame = bounds
    }

    private func loadRuntime() {
        let runtimeURL = RuntimeResourceLocator.runtimeIndexURL()
        webView.loadFileURL(runtimeURL, allowingReadAccessTo: RuntimeResourceLocator.repoRootURL())
    }
}

enum RuntimeResourceLocator {
    static func repoRootURL() -> URL {
        var url = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<2 {
            if FileManager.default.fileExists(atPath: url.appendingPathComponent("docs/00_PRD.md").path) {
                return url
            }
            url.deleteLastPathComponent()
        }
        return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    }

    static func runtimeIndexURL() -> URL {
        repoRootURL().appendingPathComponent("runtime-web/index.html")
    }
}
