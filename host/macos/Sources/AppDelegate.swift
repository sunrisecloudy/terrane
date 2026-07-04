import AppKit
import WebKit

/// The macOS host window: a native app switcher over plain HTML app UIs, with a
/// WKWebView stage and a Terrane bridge scoped to the selected app.
final class AppDelegate: NSObject, NSApplicationDelegate, WKUIDelegate, WKNavigationDelegate,
  WKDownloadDelegate
{
  private var window: NSWindow!
  private var webView: WKWebView!
  private var sourceEditor: SourceEditorPanel!
  private var sourceEditorWidthConstraint: NSLayoutConstraint!
  private var appSidebar: AppSidebarView!
  private var appSidebarWidthConstraint: NSLayoutConstraint!
  private var codeButton: NSButton!
  private var appIconView: NSImageView!
  private var appNameLabel: NSTextField!
  private var crumbSeparator: NSTextField!
  private var docField: NSTextField!
  private var bridge: TerraneBridge?
  private var sttCapture: SttCapture?
  private var sttMicButton: NSButton!
  private var sttListeningLabel: NSTextField!
  private var appSchemeHandler: AppSchemeHandler?
  private var previewSchemeHandler: PreviewSchemeHandler?
  private var home: URL!
  private var apps: [TerraneApp] = []
  private var selectedApp: TerraneApp?
  // The system-negotiated locale + the shell-chrome bundle for native strings.
  private var currentLocale = "en"
  private var chromeMessages: [String: String] = [:]

  func applicationDidFinishLaunching(_ notification: Notification) {
    home = Self.resolveHome()
    apps = AppCatalog.discover(home: home)

    let config = WKWebViewConfiguration()
    guard let bridge = TerraneBridge(home: home) else {
      fatalError("terrane-host: cannot open Terrane home at \(home.path)")
    }
    self.bridge = bridge
    // Seed the shared i18n catalog if a catalog dir is configured (parity with
    // the web host's startup seed); idempotent and best-effort. Any host or the
    // CLI seeding this home also suffices.
    if let i18nDir = ProcessInfo.processInfo.environment["TERRANE_I18N_DIR"], !i18nDir.isEmpty {
      bridge.i18nImport(path: i18nDir)
    }
    // Detect the locale from the system language once (parity with the web
    // host's Accept-Language negotiation) and load the native-chrome bundle.
    currentLocale = TerraneBridge.negotiateLocale(Locale.preferredLanguages)
    chromeMessages = bridge.i18nBundle(code: currentLocale, appId: "")
    bridge.onDocumentSet = { [weak self] name in
      DispatchQueue.main.async { self?.applyDocumentFromApp(name) }
    }
    bridge.install(into: config.userContentController)
    let appSchemeHandler = AppSchemeHandler { [weak self] in self?.apps ?? [] }
    self.appSchemeHandler = appSchemeHandler
    config.setURLSchemeHandler(appSchemeHandler, forURLScheme: AppSchemeHandler.scheme)
    let previewSchemeHandler = PreviewSchemeHandler(bridge: bridge)
    self.previewSchemeHandler = previewSchemeHandler
    config.setURLSchemeHandler(previewSchemeHandler, forURLScheme: PreviewSchemeHandler.scheme)
    webView = WKWebView(frame: .zero, configuration: config)
    webView.uiDelegate = self
    webView.navigationDelegate = self

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
      showHome()
    }

    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    true
  }

  func applicationWillTerminate(_ notification: Notification) {
    sttCapture?.stop(reason: "host-exit")
    terrane_stt_shutdown()
    bridge?.close()
    // Cached local-model engines must be released before ggml's static
    // destructors run at exit, or the process aborts.
    terrane_local_model_shutdown()
  }

  func webView(
    _ webView: WKWebView,
    decidePolicyFor navigationAction: WKNavigationAction,
    decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
  ) {
    if navigationAction.shouldPerformDownload {
      decisionHandler(.download)
      return
    }

    // A home-page card navigates to an app frame root. Route it through
    // native selection so the bridge, sidebar, and source editor follow.
    if navigationAction.targetFrame?.isMainFrame != false,
      let url = navigationAction.request.url,
      let id = HomePage.appId(for: url),
      id != selectedApp?.id,
      let app = apps.first(where: { $0.id == id })
    {
      decisionHandler(.cancel)
      select(app)
      return
    }

    decisionHandler(.allow)
  }

  func webView(
    _ webView: WKWebView,
    navigationAction: WKNavigationAction,
    didBecome download: WKDownload
  ) {
    download.delegate = self
  }

  func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
    // The shim exists once the document loads; hand it the current document
    // name + theme so terrane.onDocument / terrane.onTheme fire (parity with
    // the web host's nonce-checked hello sync).
    pushShellState()
  }

  func webView(
    _ webView: WKWebView,
    navigationResponse: WKNavigationResponse,
    didBecome download: WKDownload
  ) {
    download.delegate = self
  }

  func download(
    _ download: WKDownload,
    decideDestinationUsing response: URLResponse,
    suggestedFilename: String,
    completionHandler: @escaping (URL?) -> Void
  ) {
    let panel = NSSavePanel()
    panel.canCreateDirectories = true
    panel.nameFieldStringValue = Self.safeDownloadFilename(suggestedFilename)
    panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first

    if let window {
      panel.beginSheetModal(for: window) { result in
        completionHandler(result == .OK ? panel.url : nil)
      }
    } else {
      panel.begin { result in
        completionHandler(result == .OK ? panel.url : nil)
      }
    }
  }

  @available(macOS 12.0, *)
  func webView(
    _ webView: WKWebView,
    requestMediaCapturePermissionFor origin: WKSecurityOrigin,
    initiatedByFrame frame: WKFrameInfo,
    type: WKMediaCaptureType,
    decisionHandler: @escaping (WKPermissionDecision) -> Void
  ) {
    switch type {
    case .camera:
      decisionHandler(.prompt)
    case .microphone, .cameraAndMicrophone:
      decisionHandler(.deny)
    @unknown default:
      decisionHandler(.deny)
    }
  }

  private func buildContentView() -> NSView {
    let content = NSView()
    appSidebar = AppSidebarView()
    appSidebar.translatesAutoresizingMaskIntoConstraints = false
    appSidebar.onSelect = { [weak self] app in
      self?.select(app)
    }
    appSidebar.onHome = { [weak self] in
      self?.showHome(confirmUnsaved: true)
    }
    appSidebar.onToggleCollapse = { [weak self] in
      self?.toggleSidebar()
    }
    appSidebar.localModelPanel.configure(home: home)

    let bar = NSView()
    bar.translatesAutoresizingMaskIntoConstraints = false

    sttMicButton = NSButton(title: "🎙", target: self, action: #selector(sttMicButtonClicked(_:)))
    sttMicButton.bezelStyle = .rounded
    sttMicButton.toolTip = "Enable microphone"
    sttMicButton.translatesAutoresizingMaskIntoConstraints = false

    sttListeningLabel = NSTextField(labelWithString: "LISTENING")
    sttListeningLabel.font = .systemFont(ofSize: 11, weight: .bold)
    sttListeningLabel.textColor = .systemRed
    sttListeningLabel.isHidden = true
    sttListeningLabel.translatesAutoresizingMaskIntoConstraints = false

    codeButton = NSButton(
      title: nativeT("system.action.code", "Code"), target: self,
      action: #selector(codeButtonChanged(_:)))
    codeButton.setButtonType(.toggle)
    codeButton.bezelStyle = .rounded
    codeButton.translatesAutoresizingMaskIntoConstraints = false

    appIconView = NSImageView()
    appIconView.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 15, weight: .semibold)
    appIconView.contentTintColor = .secondaryLabelColor
    appIconView.translatesAutoresizingMaskIntoConstraints = false

    appNameLabel = NSTextField(labelWithString: "Terrane")
    appNameLabel.font = .systemFont(ofSize: 13, weight: .semibold)
    appNameLabel.textColor = .labelColor
    appNameLabel.lineBreakMode = .byTruncatingTail
    appNameLabel.translatesAutoresizingMaskIntoConstraints = false
    appNameLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

    crumbSeparator = NSTextField(labelWithString: "/")
    crumbSeparator.font = .systemFont(ofSize: 13)
    crumbSeparator.textColor = .tertiaryLabelColor
    crumbSeparator.isHidden = true
    crumbSeparator.translatesAutoresizingMaskIntoConstraints = false

    // The document name is editable in place — an app renames it via
    // terrane.setDocument, and typing here fires terrane.onDocument.
    docField = NSTextField()
    docField.isBordered = false
    docField.drawsBackground = false
    docField.font = .systemFont(ofSize: 13)
    docField.textColor = .secondaryLabelColor
    docField.placeholderString = nativeT("system.doc.untitled", "Untitled")
    docField.lineBreakMode = .byTruncatingTail
    docField.focusRingType = .none
    docField.isHidden = true
    docField.target = self
    docField.action = #selector(docFieldCommitted(_:))
    docField.translatesAutoresizingMaskIntoConstraints = false
    docField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

    sourceEditor = SourceEditorPanel()
    sourceEditor.isHidden = true
    sourceEditor.translatesAutoresizingMaskIntoConstraints = false
    sourceEditor.onSave = { [weak self] app, file, text in
      try self?.saveSource(app: app, file: file, text: text)
        ?? SourceEditorSaveResult(message: "Saved.")
    }

    bar.addSubview(appIconView)
    bar.addSubview(appNameLabel)
    bar.addSubview(crumbSeparator)
    bar.addSubview(docField)
    bar.addSubview(sttListeningLabel)
    bar.addSubview(sttMicButton)
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

      sttMicButton.trailingAnchor.constraint(equalTo: codeButton.leadingAnchor, constant: -10),
      sttMicButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      sttListeningLabel.trailingAnchor.constraint(equalTo: sttMicButton.leadingAnchor, constant: -8),
      sttListeningLabel.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      appIconView.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 16),
      appIconView.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      appIconView.widthAnchor.constraint(equalToConstant: 18),
      appIconView.heightAnchor.constraint(equalToConstant: 18),

      appNameLabel.leadingAnchor.constraint(equalTo: appIconView.trailingAnchor, constant: 8),
      appNameLabel.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      crumbSeparator.leadingAnchor.constraint(equalTo: appNameLabel.trailingAnchor, constant: 6),
      crumbSeparator.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      docField.leadingAnchor.constraint(equalTo: crumbSeparator.trailingAnchor, constant: 6),
      docField.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      docField.trailingAnchor.constraint(
        lessThanOrEqualTo: codeButton.leadingAnchor, constant: -12),

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

  /// Only an explicitly requested app id skips the landing page.
  private func initialApp() -> TerraneApp? {
    guard let requested = Self.parseAppId() else { return nil }
    return apps.first(where: { $0.id == requested })
  }

  private func select(_ app: TerraneApp, confirmUnsaved: Bool = true) {
    if confirmUnsaved, selectedApp != app, !sourceEditor.confirmDiscardIfNeeded(window: window) {
      restoreSelectedSegment()
      return
    }
    load(app)
  }

  @objc private func sttMicButtonClicked(_ sender: NSButton) {
    guard let bridge else { return }
    let appId = bridge.selectedAppId
    guard !appId.isEmpty else { return }
    if sttCapture == nil {
      sttCapture = SttCapture(handle: bridge.terraneHandle, appId: appId)
      sttCapture?.onListeningChanged = { [weak self] listening in
        DispatchQueue.main.async {
          self?.sttListeningLabel.isHidden = !listening
          self?.sttMicButton.state = listening ? .on : .off
        }
      }
    }
    if sttCapture?.isListening == true {
      sttCapture?.stop()
      return
    }
    do {
      try sttCapture?.start()
    } catch {
      let alert = NSAlert()
      alert.messageText = "Microphone unavailable"
      alert.informativeText = String(describing: error)
      alert.runModal()
    }
  }

  private func load(_ app: TerraneApp, preferredSourcePath: String? = nil) {
    if sttCapture?.isListening == true {
      sttCapture?.stop(reason: "stopped")
    }
    selectedApp = app
    bridge?.select(app: app)
    sttCapture = nil
    window.title = "\(app.name) - Terrane"

    appSidebar.select(appId: app.id)
    updateBreadcrumb(for: app)

    sourceEditor.setApp(app, preferredPath: preferredSourcePath)
    webView.load(
      URLRequest(
        url: AppSchemeHandler.frameURL(for: app),
        cachePolicy: .reloadIgnoringLocalAndRemoteCacheData
      ))
  }

  private func updateBreadcrumb(for app: TerraneApp) {
    appNameLabel.stringValue = app.name
    appIconView.image = AppSidebarView.iconImage(for: app)
    appIconView.isHidden = false
    crumbSeparator.isHidden = false
    docField.isHidden = false
    docField.stringValue = Self.storedDocName(appId: app.id)
  }

  private func hideBreadcrumb() {
    appNameLabel.stringValue = "Terrane"
    appIconView.isHidden = true
    crumbSeparator.isHidden = true
    docField.isHidden = true
    docField.stringValue = ""
  }

  /// The shared landing page: also the empty state when nothing is installed.
  private func showHome(confirmUnsaved: Bool = false) {
    if confirmUnsaved, selectedApp != nil, !sourceEditor.confirmDiscardIfNeeded(window: window) {
      restoreSelectedSegment()
      return
    }
    selectedApp = nil
    bridge?.clearSelection()
    window.title = "Terrane"
    appSidebar.select(appId: nil)
    hideBreadcrumb()
    sourceEditor?.setApp(nil)
    let emptyMessage = nativeT("system.home.emptyNative", "No plain HTML app UIs found.")
    webView.loadHTMLString(
      HomePage.render(apps: apps) ?? Self.emptyStateHTML(emptyMessage), baseURL: nil)
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
      showHome()
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

  // ---- Top-bar document / theme -------------------------------------------

  /// User edited the breadcrumb document name (Enter / focus change).
  @objc private func docFieldCommitted(_ sender: NSTextField) {
    guard let app = selectedApp else { return }
    let name = TerraneBridge.sanitizeDocName(sender.stringValue)
    Self.setStoredDocName(name, appId: app.id)
    sender.stringValue = name
    pushShellState()
  }

  /// A page renamed its own document via terrane.setDocument.
  private func applyDocumentFromApp(_ raw: String) {
    guard let app = selectedApp else { return }
    let name = TerraneBridge.sanitizeDocName(raw)
    Self.setStoredDocName(name, appId: app.id)
    docField.stringValue = name
    // Echo the canonical (sanitized) name back so the app's getDocument()/
    // onDocument converge with what we stored (its optimistic value may differ
    // after sanitization) — parity with the web host.
    pushShellState()
  }

  /// Push the current document name + theme into the loaded page. The macOS
  /// host has no in-app theme override — it always follows the OS — so the
  /// theme is "system" (parity with the web host's value vocabulary). Apps
  /// resolve the concrete appearance via CSS `color-scheme` / matchMedia,
  /// which WebKit already drives from the system appearance.
  private func pushShellState() {
    guard let app = selectedApp else { return }
    let bundle = bridge?.i18nBundle(code: currentLocale, appId: app.id) ?? [:]
    let js = TerraneBridge.applyStateJS(
      document: Self.storedDocName(appId: app.id),
      theme: "system",
      locale: currentLocale,
      messages: bundle,
      dir: TerraneBridge.dir(for: currentLocale)
    )
    webView.evaluateJavaScript(js)
  }

  /// A native-chrome string for `key` from the shell-chrome bundle, else the
  /// English `fallback`. Keys are the `system` domain, e.g. `system.doc.untitled`.
  private func nativeT(_ key: String, _ fallback: String) -> String {
    chromeMessages[key] ?? fallback
  }

  private static func docKey(_ appId: String) -> String { "terrane.doc.\(appId)" }

  static func storedDocName(appId: String) -> String {
    let value = UserDefaults.standard.string(forKey: docKey(appId)) ?? ""
    return value.isEmpty ? "Untitled" : value
  }

  static func setStoredDocName(_ name: String, appId: String) {
    UserDefaults.standard.set(name, forKey: docKey(appId))
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

  static func safeDownloadFilename(_ suggested: String) -> String {
    let trimmed = suggested.trimmingCharacters(in: .whitespacesAndNewlines)
    let name = trimmed.isEmpty ? "download" : trimmed
    let invalid = CharacterSet(charactersIn: "/:")
    return name.components(separatedBy: invalid).joined(separator: "-")
  }

  /// The empty state shown when no HTML app UIs are installed, with a localized
  /// `message` (HTML-escaped since it is dropped into the body).
  private static func emptyStateHTML(_ message: String) -> String {
    let safe = message
      .replacingOccurrences(of: "&", with: "&amp;")
      .replacingOccurrences(of: "<", with: "&lt;")
      .replacingOccurrences(of: ">", with: "&gt;")
    return """
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
      <body>\(safe)</body>
      </html>
      """
  }
}
