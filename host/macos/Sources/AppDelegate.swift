import AppKit
import WebKit

/// The macOS host window: a native app switcher over plain HTML app UIs, with a
/// WKWebView stage and a Terrane bridge scoped to the selected app.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var webView: WKWebView!
    private var appSwitcher: NSSegmentedControl!
    private var bridge: TerraneBridge?
    private var previewSchemeHandler: PreviewSchemeHandler?
    private var apps: [TerraneApp] = []
    private var selectedApp: TerraneApp?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let home = Self.resolveHome()
        apps = AppCatalog.discover(home: home)

        let config = WKWebViewConfiguration()
        guard let bridge = TerraneBridge(home: home) else {
            fatalError("terrane-host: cannot open Terrane home at \(home.path)")
        }
        self.bridge = bridge
        bridge.install(into: config.userContentController)
        let previewSchemeHandler = PreviewSchemeHandler(bridge: bridge)
        self.previewSchemeHandler = previewSchemeHandler
        config.setURLSchemeHandler(previewSchemeHandler, forURLScheme: PreviewSchemeHandler.scheme)
        webView = WKWebView(frame: .zero, configuration: config)

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 960, height: 680),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Terrane"
        window.center()
        window.contentView = buildContentView()

        renderAppSwitcher()
        if let app = initialApp() {
            select(app)
        } else {
            showNoApps()
        }

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationWillTerminate(_ notification: Notification) {
        bridge?.close()
    }

    private func buildContentView() -> NSView {
        let content = NSView()
        let bar = NSVisualEffectView()
        bar.material = .headerView
        bar.blendingMode = .withinWindow
        bar.state = .active
        bar.translatesAutoresizingMaskIntoConstraints = false

        let title = NSTextField(labelWithString: "Terrane")
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        title.setContentHuggingPriority(.required, for: .horizontal)
        title.translatesAutoresizingMaskIntoConstraints = false

        appSwitcher = NSSegmentedControl(labels: [], trackingMode: .selectOne, target: self, action: #selector(appSwitcherChanged(_:)))
        appSwitcher.segmentStyle = .rounded
        appSwitcher.translatesAutoresizingMaskIntoConstraints = false

        bar.addSubview(title)
        bar.addSubview(appSwitcher)
        content.addSubview(bar)
        content.addSubview(webView)
        webView.translatesAutoresizingMaskIntoConstraints = false

        NSLayoutConstraint.activate([
            bar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            bar.topAnchor.constraint(equalTo: content.topAnchor),
            bar.heightAnchor.constraint(equalToConstant: 48),

            title.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 16),
            title.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

            appSwitcher.leadingAnchor.constraint(equalTo: title.trailingAnchor, constant: 14),
            appSwitcher.trailingAnchor.constraint(lessThanOrEqualTo: bar.trailingAnchor, constant: -16),
            appSwitcher.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

            webView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            webView.topAnchor.constraint(equalTo: bar.bottomAnchor),
            webView.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])

        return content
    }

    private func renderAppSwitcher() {
        appSwitcher.segmentCount = apps.count
        appSwitcher.isHidden = apps.isEmpty

        for (index, app) in apps.enumerated() {
            appSwitcher.setLabel(app.name, forSegment: index)
            appSwitcher.setWidth(segmentWidth(for: app.name), forSegment: index)
            appSwitcher.setToolTip(app.id, forSegment: index)
        }
    }

    private func initialApp() -> TerraneApp? {
        let requested = Self.parseAppId()
        if let requested, let app = apps.first(where: { $0.id == requested }) {
            return app
        }
        if let app = apps.first(where: { $0.id == "todo" }) {
            return app
        }
        return apps.first
    }

    private func select(_ app: TerraneApp) {
        selectedApp = app
        bridge?.select(app: app)
        window.title = "\(app.name) - Terrane"

        if let index = apps.firstIndex(of: app) {
            appSwitcher.selectedSegment = index
        }

        webView.loadFileURL(app.uiURL, allowingReadAccessTo: app.directory)
    }

    private func showNoApps() {
        selectedApp = nil
        bridge?.clearSelection()
        window.title = "Terrane"
        webView.loadHTMLString(Self.emptyStateHTML, baseURL: nil)
    }

    private func segmentWidth(for title: String) -> CGFloat {
        min(max(CGFloat(title.count * 8 + 34), 92), 190)
    }

    @objc private func appSwitcherChanged(_ sender: NSSegmentedControl) {
        let index = sender.selectedSegment
        guard apps.indices.contains(index) else { return }
        select(apps[index])
    }

    /// Workspace home: `$TERRANE_HOME`, else `~/.terrane`. terrane-ffi appends
    /// `log.bin`.
    static func resolveHome() -> URL {
        if let home = ProcessInfo.processInfo.environment["TERRANE_HOME"], !home.isEmpty {
            return URL(fileURLWithPath: home)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".terrane")
    }

    /// Optional initial app id from `open Terrane.app --args <id>` / argv[1].
    static func parseAppId() -> String? {
        let args = CommandLine.arguments
        if args.count > 1, !args[1].hasPrefix("-") {
            return args[1]
        }
        return nil
    }

    private static let emptyStateHTML = """
    <!doctype html>
    <html>
    <head>
    <meta charset="utf-8">
    <style>
    :root { color-scheme: light dark; }
    body {
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      font: 13px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: Canvas;
      color: color-mix(in srgb, CanvasText 68%, transparent);
    }
    </style>
    </head>
    <body>No plain HTML app UIs found.</body>
    </html>
    """
}
