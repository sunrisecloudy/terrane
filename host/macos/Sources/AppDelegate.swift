import AppKit
import WebKit

/// The macOS host window: a native app switcher over plain HTML app UIs, with a
/// WKWebView stage and a Terrane bridge scoped to the selected app.
final class AppDelegate: NSObject, NSApplicationDelegate {
  private var window: NSWindow!
  private var webView: WKWebView!
  private var sourceEditor: SourceEditorPanel!
  private var sourceEditorWidthConstraint: NSLayoutConstraint!
  private var appSidebar: AppSidebarView!
  private var appSidebarWidthConstraint: NSLayoutConstraint!
  private var codeButton: NSButton!
  private var bridge: TerraneBridge?
  private var appSchemeHandler: AppSchemeHandler?
  private var previewSchemeHandler: PreviewSchemeHandler?
  private var home: URL!
  private var apps: [TerraneApp] = []
  private var selectedApp: TerraneApp?

  func applicationDidFinishLaunching(_ notification: Notification) {
    home = Self.resolveHome()
    apps = AppCatalog.discover(home: home)

    let config = WKWebViewConfiguration()
    guard let bridge = TerraneBridge(home: home) else {
      fatalError("terrane-host: cannot open Terrane home at \(home.path)")
    }
    self.bridge = bridge
    bridge.install(into: config.userContentController)
    let appSchemeHandler = AppSchemeHandler { [weak self] in self?.apps ?? [] }
    self.appSchemeHandler = appSchemeHandler
    config.setURLSchemeHandler(appSchemeHandler, forURLScheme: AppSchemeHandler.scheme)
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
      select(app, confirmUnsaved: false)
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
    // Cached local-model engines must be released before ggml's static
    // destructors run at exit, or the process aborts.
    terrane_local_model_shutdown()
  }

  private func buildContentView() -> NSView {
    let content = NSView()
    appSidebar = AppSidebarView()
    appSidebar.translatesAutoresizingMaskIntoConstraints = false
    appSidebar.onSelect = { [weak self] app in
      self?.select(app)
    }
    appSidebar.onToggleCollapse = { [weak self] in
      self?.toggleSidebar()
    }
    appSidebar.localModelPanel.configure(home: home)

    let bar = NSView()
    bar.translatesAutoresizingMaskIntoConstraints = false

    codeButton = NSButton(title: "Code", target: self, action: #selector(codeButtonChanged(_:)))
    codeButton.setButtonType(.toggle)
    codeButton.bezelStyle = .rounded
    codeButton.translatesAutoresizingMaskIntoConstraints = false

    sourceEditor = SourceEditorPanel()
    sourceEditor.isHidden = true
    sourceEditor.translatesAutoresizingMaskIntoConstraints = false
    sourceEditor.onSave = { [weak self] app, file, text in
      try self?.saveSource(app: app, file: file, text: text)
        ?? SourceEditorSaveResult(message: "Saved.")
    }

    bar.addSubview(codeButton)
    content.addSubview(appSidebar)
    content.addSubview(bar)
    content.addSubview(webView)
    content.addSubview(sourceEditor)
    webView.translatesAutoresizingMaskIntoConstraints = false
    appSidebarWidthConstraint = appSidebar.widthAnchor.constraint(equalToConstant: 224)
    sourceEditorWidthConstraint = sourceEditor.widthAnchor.constraint(equalToConstant: 0)

    NSLayoutConstraint.activate([
      appSidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
      appSidebar.topAnchor.constraint(equalTo: content.topAnchor),
      appSidebar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
      appSidebarWidthConstraint,

      bar.leadingAnchor.constraint(equalTo: appSidebar.trailingAnchor),
      bar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      bar.topAnchor.constraint(equalTo: content.topAnchor),
      bar.heightAnchor.constraint(equalToConstant: 48),

      codeButton.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -16),
      codeButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      webView.leadingAnchor.constraint(equalTo: appSidebar.trailingAnchor),
      webView.trailingAnchor.constraint(equalTo: sourceEditor.leadingAnchor),
      webView.topAnchor.constraint(equalTo: bar.bottomAnchor),
      webView.bottomAnchor.constraint(equalTo: content.bottomAnchor),

      sourceEditor.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      sourceEditor.topAnchor.constraint(equalTo: bar.bottomAnchor),
      sourceEditor.bottomAnchor.constraint(equalTo: content.bottomAnchor),
      sourceEditorWidthConstraint,
    ])

    return content
  }

  private func renderAppSwitcher() {
    appSidebar.render(apps: apps, selectedAppId: selectedApp?.id)
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

  private func select(_ app: TerraneApp, confirmUnsaved: Bool = true) {
    if confirmUnsaved, selectedApp != app, !sourceEditor.confirmDiscardIfNeeded(window: window) {
      restoreSelectedSegment()
      return
    }
    load(app)
  }

  private func load(_ app: TerraneApp, preferredSourcePath: String? = nil) {
    selectedApp = app
    bridge?.select(app: app)
    window.title = "\(app.name) - Terrane"

    appSidebar.select(appId: app.id)

    sourceEditor.setApp(app, preferredPath: preferredSourcePath)
    webView.load(
      URLRequest(
        url: AppSchemeHandler.frameURL(for: app),
        cachePolicy: .reloadIgnoringLocalAndRemoteCacheData
      ))
  }

  private func showNoApps() {
    selectedApp = nil
    bridge?.clearSelection()
    window.title = "Terrane"
    appSidebar.select(appId: nil)
    sourceEditor?.setApp(nil)
    webView.loadHTMLString(Self.emptyStateHTML, baseURL: nil)
  }

  private func saveSource(app: TerraneApp, file: SourceFile, text: String) throws
    -> SourceEditorSaveResult
  {
    try SourceEditorModel.write(text, to: file, for: app)

    if SourceEditorModel.requiresBuild(app: app) {
      let result = try TerraneBuilder.build(appDirectory: app.directory)
      try reloadAppFromDisk(id: app.id, preferredSourcePath: file.relativePath)
      return SourceEditorSaveResult(message: "Built \(result.files) files and reloaded.")
    }

    try reloadAppFromDisk(id: app.id, preferredSourcePath: file.relativePath)
    return SourceEditorSaveResult(message: "Saved and reloaded.")
  }

  private func reloadAppFromDisk(id: String, preferredSourcePath: String?) throws {
    apps = AppCatalog.discover(home: home)
    renderAppSwitcher()
    guard let app = apps.first(where: { $0.id == id }) else {
      showNoApps()
      throw SourceEditorError.invalidPath("App \(id) is no longer available.")
    }
    load(app, preferredSourcePath: preferredSourcePath)
  }

  @objc private func codeButtonChanged(_ sender: NSButton) {
    let visible = sender.state == .on
    sourceEditor.isHidden = !visible
    sourceEditorWidthConstraint.constant = visible ? 390 : 0
    window.contentView?.layoutSubtreeIfNeeded()
  }

  private func toggleSidebar() {
    let collapsed = appSidebarWidthConstraint.constant > 100
    appSidebar.setCollapsed(collapsed)
    appSidebarWidthConstraint.constant = collapsed ? 76 : 232

    NSAnimationContext.runAnimationGroup { context in
      context.duration = 0.16
      window.contentView?.layoutSubtreeIfNeeded()
    }
  }

  private func restoreSelectedSegment() {
    appSidebar.select(appId: selectedApp?.id)
  }

  /// Workspace home: `$TERRANE_HOME`, else `~/.terrane`. The host C ABI appends
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
