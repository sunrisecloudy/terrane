import AppKit
import GoogleSignIn
import TerranePremiumSession
import WebKit

private struct PremiumCatalogResponse: Decodable, Sendable {
  let apps: [PremiumCatalogEntry]
}

private struct PremiumCatalogEntry: Decodable, Sendable {
  let id: String
  let name: String?
  let publisher: String?
  let icon: String?

  var app: PremiumApp {
    PremiumApp(
      id: id,
      name: name ?? id,
      publisher: publisher ?? "Premium",
      icon: icon ?? ""
    )
  }
}

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
  private var accountButton: NSButton!
  private var appIconView: NSImageView!
  private var appNameLabel: NSTextField!
  private var crumbSeparator: NSTextField!
  private var docField: NSTextField!
  private var bridge: TerraneBridge?
  private var sttCapture: SttCapture?
  private var cameraFrameStreamer: NativeCameraFrameStreamer?
  private var nativePickCoordinator: NativePickCoordinator?
  private var visualIntakePhotoService: VisualIntakePhotoService?
  private var sttMicButton: NSButton!
  private var sttListeningLabel: NSTextField!
  private var appSchemeHandler: AppSchemeHandler?
  private var previewSchemeHandler: PreviewSchemeHandler?
  private var loopbackHost: LoopbackAppHost?
  private var mcpServer: McpLoopbackServer?
  private var home: URL!
  private var apps: [TerraneApp] = []
  private var premiumURL: URL?
  private var premiumSessionClient: PremiumSessionClient?
  private var healthAutoSync: MacHealthAutoSyncCoordinator?
  private var premiumApps: [PremiumApp] = []
  private var premiumAuthCoordinator: PremiumNativeAuthCoordinator?
  private var premiumSignInSheet: PremiumSignInSheetController?
  private var selectedApp: TerraneApp?
  // The system-negotiated locale + the shell-chrome bundle for native strings.
  private var currentLocale = "en"
  private var chromeMessages: [String: String] = [:]

  func applicationDidFinishLaunching(_ notification: Notification) {
    home = Self.resolveHome()
    setenv("TERRANE_HOME", home.path, 1)
    premiumURL = Self.resolvePremiumURL()
    apps = AppCatalog.discover(home: home)
    loopbackHost = Self.resolveRepoAppsDirectory()
      .flatMap { LoopbackAppHost(home: home, appsDirectory: $0) }

    let config = WKWebViewConfiguration()
    guard let bridge = TerraneBridge(home: home) else {
      let alert = NSAlert()
      alert.alertStyle = .critical
      alert.messageText = "Terrane could not open this workspace"
      alert.informativeText =
        TerraneBridge.lastOpenError
        ?? "The workspace at \(home.path) could not be opened."
      alert.addButton(withTitle: "Quit")
      alert.runModal()
      NSApp.terminate(nil)
      return
    }
    self.bridge = bridge
    for app in apps {
      let result = bridge.catalog(appId: app.id, name: app.name, source: app.directory.path)
      if !result.0 {
        NSLog("terrane-host: cannot catalog \(app.id) for MCP: \(result.1)")
      }
    }
    let mcpServer = McpLoopbackServer(home: home, bridge: bridge)
    do {
      try mcpServer.start()
      self.mcpServer = mcpServer
    } catch {
      NSLog("terrane-host: MCP loopback unavailable: \(error)")
    }
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
    bridge.onSidebarSectionSet = { [weak self] section in
      DispatchQueue.main.async { self?.appSidebar?.setAppSection(section) }
    }
    bridge.onPermissionRequired = { [weak self, weak bridge] prompt, completion in
      DispatchQueue.main.async {
        guard let self, let bridge else {
          completion(false)
          return
        }
        self.presentPermissionPrompt(prompt, bridge: bridge, completion: completion)
      }
    }
    bridge.onInteropPickRequired = { [weak self] prompt, completion in
      DispatchQueue.main.async {
        guard let self else {
          completion(nil)
          return
        }
        self.presentInteropPicker(prompt, completion: completion)
      }
    }
    bridge.onCameraCapturePhoto = { [weak self] completion in
      DispatchQueue.main.async {
        NativeCameraCaptureController.present(parent: self?.window) { result in
          switch result {
          case .success(let photo):
            completion(
              [
                "dataUrl": photo.dataURL,
                "mime": photo.mime,
                "width": photo.width,
                "height": photo.height,
              ],
              nil
            )
          case .failure(let error):
            completion(nil, error.localizedDescription)
          }
        }
      }
    }
    bridge.onCameraStreamStart = { [weak self] completion in
      DispatchQueue.main.async {
        guard let self else {
          completion(nil, "terrane: host is unavailable")
          return
        }
        let streamer = NativeCameraFrameStreamer()
        streamer.onFrame = { [weak self] frame in
          self?.pushCameraFrame(frame)
        }
        streamer.onError = { message in
          completion(nil, message)
        }
        self.cameraFrameStreamer?.stop()
        self.cameraFrameStreamer = streamer
        streamer.start()
        completion(["ok": true], nil)
      }
    }
    bridge.onCameraStreamStop = { [weak self] completion in
      DispatchQueue.main.async {
        self?.cameraFrameStreamer?.stop()
        self?.cameraFrameStreamer = nil
        completion(["ok": true], nil)
      }
    }
    let nativePickCoordinator = NativePickCoordinator(
      picker: PhotosNativeMediaPicker(),
      importer: TerraneBlobImporter(handle: bridge.terraneHandle)
    )
    self.nativePickCoordinator = nativePickCoordinator
    bridge.onPick = { [weak self, weak bridge] options, completion in
      DispatchQueue.main.async {
        guard let self, let bridge else {
          completion(nil, "terrane: host is unavailable")
          return
        }
        let appId = bridge.selectedAppId
        nativePickCoordinator.pick(
          options: options,
          appId: appId,
          parent: self.window,
          selectedAppId: { [weak bridge] in bridge?.selectedAppId ?? "" },
          completion: completion
        )
      }
    }
    let visualIntakePhotoService = VisualIntakePhotoService()
    self.visualIntakePhotoService = visualIntakePhotoService
    visualIntakePhotoService.onNewImage = { [weak self] result in
      self?.pushVisualIntakeImage(result)
    }
    bridge.onVisualIntakeStart = { [weak visualIntakePhotoService] completion in
      visualIntakePhotoService?.requestAccessAndStart(completion: completion)
    }
    bridge.onVisualIntakeLatest = { [weak visualIntakePhotoService] completion in
      visualIntakePhotoService?.analyzeLatest(completion: completion)
    }
    bridge.onVisualIntakeStop = { [weak visualIntakePhotoService] completion in
      visualIntakePhotoService?.stop(completion: completion)
    }
    bridge.onVisualIntakeStatus = { [weak visualIntakePhotoService] completion in
      visualIntakePhotoService?.status(completion: completion)
    }
    // Mark our app-serving custom schemes as secure contexts. WebKit only
    // exposes powerful web APIs (getUserMedia for camera/mic, etc.) on secure
    // origins, and a bare WKURLSchemeHandler scheme is not trustworthy by
    // default — so `getUserMedia` rejects with a NotAllowedError before the
    // media-capture permission delegate is ever consulted. There is no public
    // API for this; `_registerURLSchemeAsSecure:` on the process pool is the
    // established workaround. It's App-Store-disallowed, but this is a local,
    // ad-hoc-signed host, so it's acceptable here.
    Self.registerSecureSchemes(
      [AppSchemeHandler.scheme, PreviewSchemeHandler.scheme], on: config)

    bridge.install(into: config.userContentController)
    let appSchemeHandler = AppSchemeHandler(bridge: bridge) { [weak self] in self?.apps ?? [] }
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
    configurePremiumAccount()

    renderAppSwitcher()
    refreshPremiumCatalog()
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
    cameraFrameStreamer?.stop()
    visualIntakePhotoService?.stop { _, _ in }
    loopbackHost?.stop()
    mcpServer?.stop()
    terrane_stt_shutdown()
    healthAutoSync?.stop()
    bridge?.close()
    // Cached local-model engines must be released before ggml's static
    // destructors run at exit, or the process aborts.
    terrane_local_model_shutdown()
  }

  func application(_ application: NSApplication, open urls: [URL]) {
    for url in urls {
      if GIDSignIn.sharedInstance.handle(url) {
        continue
      }
      handleExternalOpen(url.absoluteString)
    }
  }

  func application(_ sender: NSApplication, openFiles filenames: [String]) {
    for filename in filenames {
      handleExternalOpen(filename)
    }
    sender.reply(toOpenOrPrint: .success)
  }

  private func handleExternalOpen(_ target: String) {
    guard let bridge else { return }
    let result = bridge.openExternal(target: target)
    if !result.0 {
      NSLog("terrane-host: cannot open external target \(target): \(result.1)")
      return
    }
    apps = AppCatalog.discover(home: home)
    renderAppSwitcher()
    if let id = Self.appId(fromExternalTarget: target),
      let app = apps.first(where: { $0.id == id })
    {
      select(app, confirmUnsaved: false)
    }
  }

  private static func appId(fromExternalTarget target: String) -> String? {
    guard let url = URL(string: target), url.scheme == "terrane" else { return nil }
    if url.host == "open" || url.host == "send" {
      return url.pathComponents.dropFirst().first
    }
    if url.host == "app" {
      return url.pathComponents.dropFirst().first
    }
    return nil
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

  func webView(
    _ webView: WKWebView,
    runOpenPanelWith parameters: WKOpenPanelParameters,
    initiatedByFrame frame: WKFrameInfo,
    completionHandler: @escaping ([URL]?) -> Void
  ) {
    let panel = NSOpenPanel()
    panel.canChooseFiles = true
    panel.canChooseDirectories = parameters.allowsDirectories
    panel.allowsMultipleSelection = parameters.allowsMultipleSelection
    panel.canCreateDirectories = false
    panel.directoryURL =
      FileManager.default.urls(for: .picturesDirectory, in: .userDomainMask).first

    let finish: (NSApplication.ModalResponse) -> Void = { response in
      completionHandler(response == .OK ? panel.urls : nil)
    }
    if let window = webView.window ?? window {
      panel.beginSheetModal(for: window, completionHandler: finish)
    } else {
      panel.begin(completionHandler: finish)
    }
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
    panel.directoryURL =
      FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first

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
    // The app must declare the capability in its manifest `browser_permissions`.
    // When it does, we `.grant` at the WebKit layer: on macOS `.prompt` is
    // unreliable and surfaces as a NotAllowedError, and macOS still presents its
    // own TCC camera/microphone consent dialog on first AVCaptureDevice use, so
    // the OS-level prompt is preserved either way.
    let permissions = Set(selectedApp?.browserPermissions ?? [])
    switch type {
    case .camera:
      decisionHandler(permissions.contains("camera") ? .grant : .deny)
    case .microphone:
      decisionHandler(permissions.contains("microphone") ? .grant : .deny)
    case .cameraAndMicrophone:
      decisionHandler(
        permissions.contains("camera") && permissions.contains("microphone") ? .grant : .deny)
    @unknown default:
      decisionHandler(.deny)
    }
  }

  /// Register the given custom schemes as secure contexts on the configuration's
  /// process pool, so WebKit exposes secure-only APIs (camera/microphone) to
  /// pages served from them. Uses the private `_registerURLSchemeAsSecure:`
  /// selector; guarded with `responds(to:)` so a WebKit change degrades to the
  /// prior (insecure) behavior instead of crashing.
  private static func registerSecureSchemes(_ schemes: [String], on config: WKWebViewConfiguration)
  {
    let pool = config.processPool
    let selector = NSSelectorFromString("_registerURLSchemeAsSecure:")
    guard pool.responds(to: selector) else {
      NSLog("terrane: _registerURLSchemeAsSecure: unavailable; camera/mic APIs may be blocked")
      return
    }
    for scheme in schemes {
      pool.perform(selector, with: scheme as NSString)
    }
  }

  private func buildContentView() -> NSView {
    let content = NSView()
    appSidebar = AppSidebarView()
    appSidebar.translatesAutoresizingMaskIntoConstraints = false
    appSidebar.onSelect = { [weak self] app in
      self?.select(app)
    }
    appSidebar.onSelectPremium = { [weak self] app in
      self?.openPremium(app)
    }
    appSidebar.onHome = { [weak self] in
      self?.showHome(confirmUnsaved: true)
    }
    appSidebar.onToggleCollapse = { [weak self] in
      self?.toggleSidebar()
    }
    appSidebar.onSectionItemSelect = { [weak self] id in
      self?.pushSidebarAction(kind: "select", id: id)
    }
    appSidebar.onSectionCreate = { [weak self] in
      self?.pushSidebarAction(kind: "create")
    }
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

    accountButton = NSButton(
      title: "Local", target: self, action: #selector(accountButtonClicked(_:)))
    accountButton.bezelStyle = .rounded
    accountButton.imagePosition = .imageLeading
    accountButton.image = NSImage(
      systemSymbolName: "person.crop.circle", accessibilityDescription: "Premium account")
    accountButton.toolTip = "Terrane Premium account"
    accountButton.isHidden = premiumURL == nil
    accountButton.translatesAutoresizingMaskIntoConstraints = false

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
    bar.addSubview(accountButton)
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

      accountButton.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -16),
      accountButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      codeButton.trailingAnchor.constraint(equalTo: accountButton.leadingAnchor, constant: -10),
      codeButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      sttMicButton.trailingAnchor.constraint(equalTo: codeButton.leadingAnchor, constant: -10),
      sttMicButton.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

      sttListeningLabel.trailingAnchor.constraint(
        equalTo: sttMicButton.leadingAnchor, constant: -8),
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
    appSidebar.render(apps: apps, premiumApps: premiumApps, selectedAppId: selectedApp?.id)
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
    cameraFrameStreamer?.stop()
    cameraFrameStreamer = nil
    selectedApp = app
    appSidebar.setAppSection(nil)
    bridge?.select(app: app)
    sttCapture = nil
    window.title = "\(app.name) - Terrane"

    appSidebar.select(appId: app.id)
    updateBreadcrumb(for: app)

    sourceEditor.setApp(app, preferredPath: preferredSourcePath)
    let frameURL =
      shouldUseLoopbackFrame(for: app)
      ? loopbackHost?.frameURL(for: app) ?? AppSchemeHandler.frameURL(for: app)
      : AppSchemeHandler.frameURL(for: app)
    webView.load(URLRequest(url: frameURL, cachePolicy: .reloadIgnoringLocalAndRemoteCacheData))
  }

  private func shouldUseLoopbackFrame(for app: TerraneApp) -> Bool {
    let permissions = Set(app.browserPermissions)
    return permissions.contains("camera") || permissions.contains("microphone")
  }

  private func pushCameraFrame(_ frame: NativeCameraFrame) {
    let payload: [String: Any] = [
      "dataUrl": frame.dataURL,
      "mime": frame.mime,
      "width": frame.width,
      "height": frame.height,
    ]
    guard
      let data = try? JSONSerialization.data(withJSONObject: payload),
      let json = String(data: data, encoding: .utf8)
    else {
      return
    }
    webView.evaluateJavaScript(
      "window.dispatchEvent(new CustomEvent('terrane:cameraFrame', { detail: \(json) }));")
  }

  private func pushVisualIntakeImage(_ payload: [String: Any]) {
    guard bridge?.selectedAppId == "visual-intake",
      let data = try? JSONSerialization.data(withJSONObject: payload),
      let json = String(data: data, encoding: .utf8)
    else {
      return
    }
    webView.evaluateJavaScript(
      "window.dispatchEvent(new CustomEvent('terrane:visualIntakeImage', { detail: \(json) }));")
  }

  private func openPremium(_ app: PremiumApp) {
    guard let url = premiumDashboardURL(for: app) else { return }
    if selectedApp != nil, !sourceEditor.confirmDiscardIfNeeded(window: window) {
      restoreSelectedSegment()
      return
    }
    if sttCapture?.isListening == true {
      sttCapture?.stop(reason: "stopped")
    }
    selectedApp = nil
    appSidebar.setAppSection(nil)
    bridge?.clearSelection()
    sttCapture = nil
    window.title = "\(app.name) - Terrane Premium"
    appSidebar.select(appId: "premium:\(app.id)")
    updatePremiumBreadcrumb(for: app)
    sourceEditor.setApp(nil)
    codeButton.state = .off
    sourceEditor.isHidden = true
    sourceEditorWidthConstraint.constant = 0
    webView.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalAndRemoteCacheData))
  }

  private func premiumDashboardURL(for app: PremiumApp) -> URL? {
    guard let premiumURL else { return nil }
    var components = URLComponents(
      url: premiumURL.appendingPathComponent("apps.html"), resolvingAgainstBaseURL: false)
    components?.fragment = app.dashboardFragment
    return components?.url
  }

  private func updatePremiumBreadcrumb(for app: PremiumApp) {
    appNameLabel.stringValue = app.name
    appIconView.image = AppSidebarView.iconImage(for: app)
    appIconView.isHidden = false
    crumbSeparator.isHidden = true
    docField.isHidden = true
    docField.stringValue = ""
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

  private func configurePremiumAccount() {
    guard let premiumURL else {
      accountButton?.isHidden = true
      return
    }
    let version =
      Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
      ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
      ?? "development"
    let device = PremiumDeviceMetadata(
      platform: .macOS,
      deviceName: Host.current().localizedName ?? "Mac",
      clientVersion: version
    )
    do {
      let tokenStore: any PremiumRefreshTokenStore
      #if DEBUG
        if let refreshToken = ProcessInfo.processInfo.environment[
          "TERRANE_E2E_PREMIUM_REFRESH_TOKEN"
        ], !refreshToken.isEmpty {
          tokenStore = PremiumVolatileRefreshTokenStore(refreshToken: refreshToken)
        } else {
          tokenStore = PremiumKeychainRefreshTokenStore(
            service: "com.terrane.host.premium-session"
          )
        }
      #else
        tokenStore = PremiumKeychainRefreshTokenStore(
          service: "com.terrane.host.premium-session"
        )
      #endif
      let client = try PremiumSessionClient(
        baseURL: premiumURL,
        device: device,
        tokenStore: tokenStore,
        stateObserver: { [weak self] state in
          DispatchQueue.main.async {
            self?.updatePremiumAccountControl(state)
            if case .offline(let context) = state {
              NSLog("terrane-host: Premium restore offline: \(context.message)")
              #if DEBUG
                if let errorPath = ProcessInfo.processInfo.environment[
                  "TERRANE_E2E_PREMIUM_ERROR_PATH"
                ], !errorPath.isEmpty {
                  try? context.message.write(
                    to: URL(fileURLWithPath: errorPath),
                    atomically: true,
                    encoding: .utf8
                  )
                }
              #endif
            }
            if case .signedIn = state {
              self?.refreshPremiumCatalog()
              if let session = self?.premiumSessionClient {
                self?.startHealthAutoSync(session: session)
              }
            } else if case .revoked = state {
              self?.healthAutoSync?.stop()
            }
          }
        }
      )
      premiumSessionClient = client
      let coordinator = PremiumNativeAuthCoordinator(window: window, session: client)
      premiumAuthCoordinator = coordinator
      coordinator.onCompletion = { [weak self] result in
        guard case .failure(let error) = result,
          (error as? PremiumNativeAuthError) != .cancelled
        else { return }
        self?.presentPremiumError(error)
      }
      premiumSignInSheet = PremiumSignInSheetController(parent: window) {
        [weak coordinator] provider in
        coordinator?.signIn(with: provider)
      }
      updatePremiumAccountControl(.signedOut)
      Task {
        #if DEBUG
          if let refreshToken = ProcessInfo.processInfo.environment[
            "TERRANE_E2E_PREMIUM_REFRESH_TOKEN"
          ], !refreshToken.isEmpty {
            try? await tokenStore.save(refreshToken)
          }
        #endif
        await client.restoreSession()
      }
    } catch {
      accountButton.title = "Premium unavailable"
      accountButton.isEnabled = false
      NSLog("terrane-host: Premium session client unavailable: \(error.localizedDescription)")
    }
  }

  private func startHealthAutoSync(session: PremiumSessionClient) {
    guard let bridge else { return }
    if healthAutoSync == nil {
      #if DEBUG
        if ProcessInfo.processInfo.environment["TERRANE_E2E_HEALTH_AUTO_GRANT"] == "1" {
          for namespace in ["kv", "blob", "model"] {
            _ = bridge.grant(app: "health", namespace: namespace)
          }
        }
      #endif
      let coordinator = MacHealthAutoSyncCoordinator(session: session, bridge: bridge)
      coordinator.onNutritionReady = { [weak self] mealID, _ in
        guard let self,
          let health = self.apps.first(where: { $0.id == "health" })
        else { return }
        self.select(health, confirmUnsaved: false)
        self.webView.evaluateJavaScript(
          "window.location.hash = '#/meal/\(mealID)';"
        )
      }
      healthAutoSync = coordinator
    }
    healthAutoSync?.start()
  }

  private func updatePremiumAccountControl(_ state: PremiumSessionState) {
    guard let accountButton else { return }
    accountButton.isHidden = premiumURL == nil
    accountButton.isEnabled = true
    accountButton.image = NSImage(
      systemSymbolName: "person.crop.circle", accessibilityDescription: "Premium account")
    switch state {
    case .signedOut:
      accountButton.title = "Enable Sync"
      accountButton.toolTip = "Enable optional Terrane sync and Premium services"
    case .refreshing(_):
      accountButton.title = "Connecting…"
      accountButton.isEnabled = false
    case .authenticating(let context):
      accountButton.title = context.provider == .apple ? "Apple…" : "Google…"
      accountButton.isEnabled = false
    case .signedIn(let account):
      accountButton.title =
        account.displayName ?? account.email ?? "Premium"
      accountButton.image = NSImage(
        systemSymbolName: "person.crop.circle.fill", accessibilityDescription: "Signed in")
      accountButton.toolTip = account.email ?? "Terrane Premium account"
    case .offline(let context):
      accountButton.title =
        context.account?.displayName ?? context.account?.email ?? "Offline"
      accountButton.image = NSImage(
        systemSymbolName: "person.crop.circle.badge.exclamationmark",
        accessibilityDescription: "Premium account offline")
      accountButton.toolTip = "Premium is offline; local Terrane apps remain available"
    case .revoked:
      accountButton.title = "Enable Sync"
      accountButton.toolTip = "Your Premium session ended. Enable sync to sign in again."
    }
  }

  @objc private func accountButtonClicked(_ sender: NSButton) {
    guard let client = premiumSessionClient else { return }
    Task { [weak self, weak sender] in
      let state = await client.state
      await MainActor.run {
        guard let self, let sender else { return }
        switch state {
        case .signedIn(let account), .refreshing(let account?):
          self.presentPremiumAccountMenu(account: account, sender: sender)
        case .offline(let context) where context.account != nil:
          self.presentPremiumAccountMenu(account: context.account!, sender: sender)
        default:
          self.premiumSignInSheet?.present()
        }
      }
    }
  }

  private func presentPremiumAccountMenu(account: PremiumAccount, sender: NSButton) {
    let menu = NSMenu()
    let identity = NSMenuItem(
      title: account.displayName ?? account.email ?? "Terrane Premium",
      action: nil,
      keyEquivalent: ""
    )
    identity.isEnabled = false
    menu.addItem(identity)
    if let email = account.email, account.displayName != nil {
      let emailItem = NSMenuItem(title: email, action: nil, keyEquivalent: "")
      emailItem.isEnabled = false
      menu.addItem(emailItem)
    }
    menu.addItem(.separator())

    let refresh = NSMenuItem(
      title: "Refresh Session", action: #selector(refreshPremiumSession), keyEquivalent: "")
    refresh.target = self
    menu.addItem(refresh)

    for provider in PremiumIdentityProvider.allCases
    where !account.linkedProviders.contains(provider) {
      let item = NSMenuItem(
        title: "Link \(provider == .apple ? "Apple" : "Google")",
        action: provider == .apple ? #selector(linkPremiumApple) : #selector(linkPremiumGoogle),
        keyEquivalent: ""
      )
      item.target = self
      menu.addItem(item)
    }
    menu.addItem(.separator())
    let logout = NSMenuItem(
      title: "Sign Out", action: #selector(logoutPremium), keyEquivalent: "")
    logout.target = self
    menu.addItem(logout)
    if let event = NSApp.currentEvent {
      NSMenu.popUpContextMenu(menu, with: event, for: sender)
    } else {
      menu.popUp(
        positioning: nil,
        at: NSPoint(x: 0, y: sender.bounds.maxY + 4),
        in: sender
      )
    }
  }

  @objc private func refreshPremiumSession() {
    guard let client = premiumSessionClient else { return }
    Task { [weak self] in
      do {
        _ = try await client.refresh(force: true)
      } catch {
        await MainActor.run {
          if !Self.isExpectedOfflineError(error) {
            self?.presentPremiumError(error)
          }
        }
      }
    }
  }

  @objc private func linkPremiumApple() {
    premiumAuthCoordinator?.link(.apple)
  }

  @objc private func linkPremiumGoogle() {
    premiumAuthCoordinator?.link(.google)
  }

  @objc private func logoutPremium() {
    guard let client = premiumSessionClient else { return }
    Task { [weak self] in
      do {
        try await client.logout()
      } catch {
        await MainActor.run {
          if !Self.isExpectedOfflineError(error) {
            self?.presentPremiumError(error)
          }
        }
      }
      await MainActor.run { self?.refreshPremiumCatalog() }
    }
  }

  private func presentPremiumError(_ error: Error) {
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = "Terrane Premium"
    alert.informativeText = error.localizedDescription
    alert.addButton(withTitle: "OK")
    if let window {
      alert.beginSheetModal(for: window)
    } else {
      alert.runModal()
    }
  }

  private static func isExpectedOfflineError(_ error: Error) -> Bool {
    if case .transport = error as? PremiumSessionError {
      return true
    }
    let error = error as NSError
    return error.domain == NSURLErrorDomain
      && [
        NSURLErrorNotConnectedToInternet,
        NSURLErrorNetworkConnectionLost,
        NSURLErrorCannotConnectToHost,
        NSURLErrorCannotFindHost,
        NSURLErrorTimedOut,
      ].contains(error.code)
  }

  /// The shared landing page: also the empty state when nothing is installed.
  private func showHome(confirmUnsaved: Bool = false) {
    if confirmUnsaved, selectedApp != nil, !sourceEditor.confirmDiscardIfNeeded(window: window) {
      restoreSelectedSegment()
      return
    }
    selectedApp = nil
    appSidebar.setAppSection(nil)
    bridge?.clearSelection()
    window.title = "Terrane"
    appSidebar.select(appId: nil)
    hideBreadcrumb()
    sourceEditor?.setApp(nil)
    let emptyMessage = nativeT("system.home.emptyNative", "No plain HTML app UIs found.")
    webView.loadHTMLString(
      HomePage.render(apps: apps) ?? Self.emptyStateHTML(emptyMessage), baseURL: nil)
  }

  private func refreshPremiumCatalog() {
    guard premiumURL != nil else { return }
    guard let client = premiumSessionClient else {
      refreshPublicPremiumCatalog()
      return
    }
    Task { [weak self] in
      let state = await client.state
      if case .signedIn = state {
        do {
          let response: PremiumCatalogResponse = try await client.send(
            path: "marketplace/premium-apps")
          await MainActor.run {
            self?.premiumApps = response.apps.map(\.app)
            self?.renderAppSwitcher()
          }
          return
        } catch {
          NSLog(
            "terrane-host: authenticated Premium catalog unavailable: \(error.localizedDescription)"
          )
        }
      }
      await MainActor.run { self?.refreshPublicPremiumCatalog() }
    }
  }

  private func refreshPublicPremiumCatalog() {
    guard let premiumURL else { return }
    URLSession.shared.dataTask(
      with: premiumURL.appendingPathComponent("marketplace/premium-apps")
    ) { [weak self] data, _, _ in
      let apps = data.map(PremiumCatalog.parse) ?? []
      DispatchQueue.main.async {
        self?.premiumApps = apps
        self?.renderAppSwitcher()
      }
    }.resume()
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

  private func presentPermissionPrompt(
    _ prompt: PermissionRequiredPrompt,
    bridge: TerraneBridge,
    completion: @escaping (Bool) -> Void
  ) {
    let alert = NSAlert()
    alert.messageText = "Allow \(prompt.appName) to access resources?"
    alert.informativeText =
      "The app is requesting: \(prompt.missingResources.joined(separator: ", "))."
    alert.alertStyle = .warning
    alert.addButton(withTitle: "Allow")
    alert.addButton(withTitle: "Deny")

    let handleDecision: (NSApplication.ModalResponse) -> Void = {
      [weak self, weak bridge] response in
      guard response == .alertFirstButtonReturn else {
        completion(false)
        return
      }
      guard let bridge else {
        completion(false)
        return
      }
      for namespace in prompt.missingResources {
        let result = bridge.grant(app: prompt.appId, namespace: namespace)
        guard result.0 else {
          self?.presentGrantFailure(namespace: namespace, detail: result.1)
          completion(false)
          return
        }
      }
      completion(true)
    }

    if let window {
      alert.beginSheetModal(for: window, completionHandler: handleDecision)
    } else {
      handleDecision(alert.runModal())
    }
  }

  private func presentInteropPicker(
    _ prompt: InteropPickPrompt,
    completion: @escaping (String?) -> Void
  ) {
    let appLabel = prompt.app.isEmpty ? "An app" : prompt.app
    let alert = NSAlert()
    alert.messageText = "Choose an app for \(appLabel)"
    alert.alertStyle = .informational

    guard !prompt.candidates.isEmpty else {
      alert.informativeText =
        "No installed app can receive over the \(prompt.interface) interface yet."
      alert.addButton(withTitle: "OK")
      let finish: (NSApplication.ModalResponse) -> Void = { _ in completion(nil) }
      if let window {
        alert.beginSheetModal(for: window, completionHandler: finish)
      } else {
        finish(alert.runModal())
      }
      return
    }

    alert.informativeText =
      "\(appLabel) wants to hand off over the \(prompt.interface) interface."
    let popup = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
    for candidate in prompt.candidates {
      let title =
        candidate.name.isEmpty ? candidate.id : "\(candidate.name) (\(candidate.id))"
      popup.addItem(withTitle: title)
      popup.lastItem?.representedObject = candidate.id
    }
    alert.accessoryView = popup
    alert.addButton(withTitle: "Choose")
    alert.addButton(withTitle: "Cancel")

    let handleDecision: (NSApplication.ModalResponse) -> Void = { response in
      guard response == .alertFirstButtonReturn,
        let target = popup.selectedItem?.representedObject as? String
      else {
        completion(nil)
        return
      }
      completion(target)
    }

    if let window {
      alert.beginSheetModal(for: window, completionHandler: handleDecision)
    } else {
      handleDecision(alert.runModal())
    }
  }

  private func presentGrantFailure(namespace: String, detail: String) {
    let alert = NSAlert()
    alert.messageText = "Permission grant failed"
    alert.informativeText = "Could not grant \(namespace): \(detail)"
    alert.alertStyle = .critical
    if let window {
      alert.beginSheetModal(for: window)
    } else {
      alert.runModal()
    }
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

  private func pushSidebarAction(kind: String, id: String? = nil) {
    var payload: [String: String] = ["kind": kind]
    if let id { payload["id"] = id }
    guard
      let data = try? JSONSerialization.data(withJSONObject: payload),
      let json = String(data: data, encoding: .utf8)
    else {
      return
    }
    webView.evaluateJavaScript(
      "window.__terrane_sidebar_action && window.__terrane_sidebar_action(\(json));")
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

  static func resolveRepoAppsDirectory() -> URL? {
    if let repo = ProcessInfo.processInfo.environment["TERRANE_REPO"]?
      .trimmingCharacters(in: .whitespacesAndNewlines),
      !repo.isEmpty
    {
      return URL(fileURLWithPath: repo).appendingPathComponent("apps")
    }
    let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    let apps = cwd.appendingPathComponent("apps")
    return FileManager.default.fileExists(atPath: apps.path) ? apps : nil
  }

  /// Optional initial app id from `open Terrane.app --args <id>` / argv[1].
  static func parseAppId() -> String? {
    let args = CommandLine.arguments
    if args.count > 1, !args[1].hasPrefix("-") {
      return args[1]
    }
    return nil
  }

  static func resolvePremiumURL() -> URL? {
    let args = CommandLine.arguments
    let cliValue = args.indices.dropLast().first { args[$0] == "--premium-url" }
      .map { args[$0 + 1] }
    let raw = cliValue ?? ProcessInfo.processInfo.environment["TERRANE_PREMIUM_URL"]
    guard
      let trimmed = raw?.trimmingCharacters(in: .whitespacesAndNewlines)
        .trimmingCharacters(in: CharacterSet(charactersIn: "/")),
      !trimmed.isEmpty
    else {
      return nil
    }
    return URL(string: trimmed)
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
    let safe =
      message
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
