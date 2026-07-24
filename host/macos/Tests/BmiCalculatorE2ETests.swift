import Foundation
import WebKit
import XCTest

final class BmiCalculatorE2ETests: XCTestCase {
  func testPermissionRequiredPromptParsesHostMessage() throws {
    let prompt = try XCTUnwrap(
      PermissionRequiredPrompt.parse(
        error:
          "permission required for app chat: grant kv, local-model; open http://127.0.0.1:8780/__terrane/admin/requests/local-chat-user-local-owner-kv_local-model",
        appId: "chat",
        appName: "Chat"
      )
    )
    XCTAssertEqual(prompt.appId, "chat")
    XCTAssertEqual(prompt.appName, "Chat")
    XCTAssertEqual(prompt.missingResources, ["kv", "local-model"])

    XCTAssertNil(
      PermissionRequiredPrompt.parse(
        error: "permission required for app other: grant kv; open http://127.0.0.1:8780",
        appId: "chat",
        appName: "Chat"
      ))
  }

  func testBmiCalculatorCatalogAssetsShimAndBridgeInvoke() throws {
    let fixture = try BmiFixture()
    defer { fixture.cleanUp() }

    let apps = AppCatalog.discover(home: fixture.home)
    let app = try XCTUnwrap(
      apps.first { $0.id == "bmi-calculator" },
      "BMI Calculator should be discovered from the temporary Terrane home"
    )
    XCTAssertEqual(app.name, "BMI Calculator")
    XCTAssertEqual(app.directory.lastPathComponent, "bmi-calculator")
    XCTAssertEqual(app.uiURL.lastPathComponent, "index.html")
    XCTAssertTrue(app.uiURL.path.contains("/dist/"))
    XCTAssertEqual(app.iconPath, "icon.svg")
    XCTAssertEqual(app.iconURL?.lastPathComponent, "icon.svg")

    XCTAssertTrue(SourceEditorModel.requiresBuild(app: app))
    let sourceFiles = try SourceEditorModel.editableFiles(for: app)
    let sourcePaths = sourceFiles.map(\.relativePath)
    XCTAssertTrue(sourcePaths.contains("manifest.json"), sourcePaths.joined(separator: ","))
    XCTAssertTrue(sourcePaths.contains("src/main.tsx"), sourcePaths.joined(separator: ","))
    XCTAssertTrue(sourcePaths.contains("src/app.css"), sourcePaths.joined(separator: ","))
    XCTAssertFalse(
      sourcePaths.contains { $0.hasPrefix("dist/") }, sourcePaths.joined(separator: ","))

    let html = try String(contentsOf: app.uiURL, encoding: .utf8)
    XCTAssertTrue(html.contains("<title>BMI Calculator</title>"), html)
    XCTAssertTrue(html.contains("assets/app.css"), html)
    XCTAssertTrue(html.contains("assets/react.production.min.js"), html)
    XCTAssertTrue(html.contains("assets/react-dom.production.min.js"), html)
    XCTAssertTrue(html.contains("assets/modules/src/main.js"), html)

    let assets = app.uiURL.deletingLastPathComponent().appendingPathComponent("assets")
    let css = assets.appendingPathComponent("app.css")
    let module = assets.appendingPathComponent("modules/src/main.js")
    let jsxRuntime = assets.appendingPathComponent("terrane-react-jsx-runtime.js")
    XCTAssertTrue(FileManager.default.fileExists(atPath: css.path), css.path)
    XCTAssertTrue(FileManager.default.fileExists(atPath: module.path), module.path)
    XCTAssertTrue(FileManager.default.fileExists(atPath: jsxRuntime.path), jsxRuntime.path)

    let moduleSource = try String(contentsOf: module, encoding: .utf8)
    XCTAssertTrue(moduleSource.contains("terrane.invoke"), moduleSource)
    XCTAssertTrue(moduleSource.contains("id: \"height\""), moduleSource)
    XCTAssertTrue(moduleSource.contains("id: \"weight\""), moduleSource)
    XCTAssertTrue(moduleSource.contains("id: \"bmi-value\""), moduleSource)
    XCTAssertTrue(moduleSource.contains("createRoot"), moduleSource)

    let mainSource = try XCTUnwrap(sourceFiles.first { $0.relativePath == "src/main.tsx" })
    let originalMain = try SourceEditorModel.read(mainSource)
    let editedMain = originalMain.replacingOccurrences(
      of: "BMI Calculator",
      with: "BMI Calculator Live Edit"
    )
    XCTAssertNotEqual(originalMain, editedMain)
    try SourceEditorModel.write(editedMain, to: mainSource, for: app)
    let build = try TerraneBuilder.build(appDirectory: app.directory)
    XCTAssertGreaterThan(build.files, 0)
    XCTAssertTrue(build.dist.path.hasSuffix("/dist"), build.dist.path)

    let rebuiltModule = try String(contentsOf: module, encoding: .utf8)
    XCTAssertTrue(rebuiltModule.contains("BMI Calculator Live Edit"), rebuiltModule)

    let frameAsset = AppAssetStore.asset(apps: [app], appId: "bmi-calculator", relPath: "")
    guard case .success(let frame) = frameAsset else {
      XCTFail("BMI frame should be served by terrane-app scheme")
      return
    }
    XCTAssertEqual(frame.contentType, "text/html; charset=utf-8")
    XCTAssertTrue(
      String(data: frame.data, encoding: .utf8)?.contains("assets/modules/src/main.js") == true)

    let moduleAsset = AppAssetStore.asset(
      apps: [app], appId: "bmi-calculator", relPath: "assets/modules/src/main.js")
    guard case .success(let servedModule) = moduleAsset else {
      XCTFail("BMI module should be served by terrane-app scheme")
      return
    }
    XCTAssertEqual(servedModule.contentType, "text/javascript; charset=utf-8")
    XCTAssertTrue(String(data: servedModule.data, encoding: .utf8)?.contains("createRoot") == true)

    let iconAsset = AppAssetStore.asset(
      apps: [app], appId: "bmi-calculator", relPath: "icon.svg", base: .appRoot)
    guard case .success(let servedIcon) = iconAsset else {
      XCTFail("BMI icon should be served from the app root")
      return
    }
    XCTAssertEqual(servedIcon.contentType, "image/svg+xml; charset=utf-8")
    XCTAssertTrue(String(data: servedIcon.data, encoding: .utf8)?.contains("currentColor") == true)

    let bridge = try XCTUnwrap(TerraneBridge(home: fixture.home))
    defer { bridge.close() }

    let userContent = WKUserContentController()
    bridge.install(into: userContent)
    XCTAssertEqual(userContent.userScripts.count, 1)
    XCTAssertTrue(
      userContent.userScripts[0].source.contains(#"Object.defineProperty(window, "terrane""#))
    XCTAssertTrue(userContent.userScripts[0].source.contains("previewInvoke"))
    XCTAssertTrue(userContent.userScripts[0].source.contains("builderGenerate"))

    bridge.select(app: app)
    let grant = bridge.grant(app: app.id, namespace: "kv")
    XCTAssertTrue(grant.0, grant.1)
    let result = bridge.invokeSelectedApp(
      verb: "calculate",
      args: ["180", "81"]
    )
    XCTAssertTrue(result.0, result.1)
    XCTAssertTrue(result.1.contains(#""bmi":25"#), result.1)
    XCTAssertTrue(result.1.contains(#""category":"Overweight""#), result.1)
  }

  func testSourceEditorPanelDisplaysCodeAndSavesSelectedFile() throws {
    try runOnMainThread {
      let fixture = try BmiFixture()
      defer { fixture.cleanUp() }

      let apps = AppCatalog.discover(home: fixture.home)
      let app = try XCTUnwrap(apps.first { $0.id == "bmi-calculator" })
      let panel = SourceEditorPanel(frame: NSRect(x: 0, y: 0, width: 390, height: 640))

      panel.setApp(app, preferredPath: "src/main.tsx")
      panel.layoutSubtreeIfNeeded()

      let fileMenu = try XCTUnwrap(firstSubview(ofType: NSPopUpButton.self, in: panel))
      XCTAssertTrue(
        fileMenu.itemTitles.contains("manifest.json"), fileMenu.itemTitles.joined(separator: ","))
      XCTAssertTrue(
        fileMenu.itemTitles.contains("src/app.css"), fileMenu.itemTitles.joined(separator: ","))
      XCTAssertTrue(
        fileMenu.itemTitles.contains("src/main.tsx"), fileMenu.itemTitles.joined(separator: ","))
      XCTAssertEqual(fileMenu.titleOfSelectedItem, "src/main.tsx")

      let scrollView = try XCTUnwrap(firstSubview(ofType: NSScrollView.self, in: panel))
      let textView = try XCTUnwrap(scrollView.documentView as? NSTextView)
      XCTAssertFalse(scrollView.hasHorizontalScroller)
      XCTAssertTrue(textView.drawsBackground)
      XCTAssertNotNil(textView.textColor)
      XCTAssertFalse(textView.string.isEmpty)
      XCTAssertTrue(textView.string.contains("BMI Calculator"), textView.string)
      XCTAssertEqual(textView.selectedRange().location, 0)
      XCTAssertEqual(scrollView.contentView.bounds.origin.x, 0)
      assertForegroundColor(
        in: textView,
        matching: "import",
        equals: .systemPurple
      )
      assertForegroundColor(
        in: textView,
        matching: "\"react\"",
        equals: .systemGreen
      )

      let saveButton = try XCTUnwrap(
        subviews(ofType: NSButton.self, in: panel).first { $0.title == "Save & Reload" })
      XCTAssertFalse(saveButton.isEnabled)

      var savedPath: String?
      var savedText: String?
      panel.onSave = { _, file, text in
        savedPath = file.relativePath
        savedText = text
        return SourceEditorSaveResult(message: "Saved from test.")
      }

      textView.string += "\n// view-level save check"
      panel.textDidChange(Notification(name: NSText.didChangeNotification, object: textView))
      XCTAssertTrue(saveButton.isEnabled)

      saveButton.performClick(nil)
      XCTAssertEqual(savedPath, "src/main.tsx")
      XCTAssertTrue(savedText?.contains("view-level save check") == true)
      XCTAssertFalse(saveButton.isEnabled)
    }
  }

  func testAppSidebarRendersAppsAndTracksSelection() throws {
    try runOnMainThread {
      let base = URL(fileURLWithPath: "/tmp/terrane-sidebar-test")
      let todo = TerraneApp(
        id: "todo",
        name: "Todo",
        directory: base.appendingPathComponent("todo"),
        uiURL: base.appendingPathComponent("todo/index.html"),
        iconPath: nil,
        iconURL: nil,
        browserPermissions: []
      )
      let paint = TerraneApp(
        id: "pixel-paint",
        name: "Pixel Paint",
        directory: base.appendingPathComponent("pixel-paint"),
        uiURL: base.appendingPathComponent("pixel-paint/index.html"),
        iconPath: nil,
        iconURL: nil,
        browserPermissions: []
      )

      let sidebar = AppSidebarView(frame: NSRect(x: 0, y: 0, width: 224, height: 640))
      var selected: TerraneApp?
      var selectedPremium: PremiumApp?
      var wentHome = false
      sidebar.onSelect = { selected = $0 }
      sidebar.onSelectPremium = { selectedPremium = $0 }
      sidebar.onHome = { wentHome = true }
      sidebar.render(
        apps: [paint, todo],
        premiumApps: [
          PremiumApp(id: "todo", name: "Premium Todo", publisher: "Terrane Premium", icon: ""),
          PremiumApp(
            id: "hold-dear-route-shop", name: "Hold Dear Route Shop",
            publisher: "Hold Dear Studio", icon: ""),
        ],
        selectedAppId: "todo"
      )
      sidebar.layoutSubtreeIfNeeded()

      // Home leads the stack, then discovered apps, then premium apps not installed locally.
      let buttons = subviews(ofType: AppSidebarButton.self, in: sidebar)
      XCTAssertEqual(buttons.map(\.title), ["Home", "Pixel Paint", "Todo", "Hold Dear Route Shop"])
      XCTAssertEqual(buttons.map(\.toolTip), ["", "pixel-paint", "todo", "hold-dear-route-shop"])
      XCTAssertEqual(buttons.map(\.isSelected), [false, false, true, false])
      XCTAssertTrue(buttons.allSatisfy { $0.image != nil })
      XCTAssertTrue(subviews(ofType: LocalModelPanel.self, in: sidebar).isEmpty)

      sidebar.selectApp(at: 0)
      XCTAssertEqual(selected?.id, "pixel-paint")

      sidebar.select(appId: "pixel-paint")
      XCTAssertEqual(buttons.map(\.isSelected), [false, true, false, false])

      buttons[0].performClick(nil)
      XCTAssertTrue(wentHome, "home button should fire onHome")
      buttons[3].performClick(nil)
      XCTAssertEqual(selectedPremium?.id, "hold-dear-route-shop")
      sidebar.select(appId: nil)
      XCTAssertEqual(buttons.map(\.isSelected), [true, false, false, false])

      sidebar.setCollapsed(true)
      XCTAssertEqual(buttons.map(\.title), ["", "", "", ""])
      XCTAssertEqual(buttons.map(\.toolTip), ["Home", "Pixel Paint", "Todo", "Hold Dear Route Shop"])

      sidebar.setCollapsed(false)
      XCTAssertEqual(buttons.map(\.title), ["Home", "Pixel Paint", "Todo", "Hold Dear Route Shop"])
    }
  }

  func testAppSidebarResetsAppsScrollOnlyWhenSelectionChanges() throws {
    try runOnMainThread {
      let base = URL(fileURLWithPath: "/tmp/terrane-sidebar-scroll-test")
      let apps = (0..<16).map { index in
        TerraneApp(
          id: "app-\(index)",
          name: "App \(index)",
          directory: base.appendingPathComponent("app-\(index)"),
          uiURL: base.appendingPathComponent("app-\(index)/index.html"),
          iconPath: nil,
          iconURL: nil,
          browserPermissions: []
        )
      }
      let sidebar = AppSidebarView(frame: NSRect(x: 0, y: 0, width: 232, height: 420))
      sidebar.render(apps: apps, selectedAppId: "app-0")
      sidebar.layoutSubtreeIfNeeded()

      let appsScroll = try XCTUnwrap(
        subviews(ofType: NSScrollView.self, in: sidebar).first { scrollView in
          guard let documentView = scrollView.documentView else { return false }
          return !subviews(ofType: AppSidebarButton.self, in: documentView).isEmpty
        })
      appsScroll.layoutSubtreeIfNeeded()
      let documentView = try XCTUnwrap(appsScroll.documentView)
      documentView.layoutSubtreeIfNeeded()
      let appButtons = subviews(ofType: AppSidebarButton.self, in: documentView)
      XCTAssertTrue(documentView.isFlipped)
      XCTAssertLessThan(try XCTUnwrap(appButtons.first).frame.minY, try XCTUnwrap(appButtons.last).frame.minY)
      let topY =
        documentView.isFlipped
        ? CGFloat(0)
        : max(0, documentView.bounds.height - appsScroll.contentView.bounds.height)
      let awayFromTopY =
        documentView.isFlipped
        ? max(0, documentView.bounds.height - appsScroll.contentView.bounds.height)
        : CGFloat(0)
      appsScroll.contentView.scroll(to: NSPoint(x: 0, y: awayFromTopY))
      appsScroll.reflectScrolledClipView(appsScroll.contentView)
      let deliberateScrollY = appsScroll.contentView.bounds.origin.y
      XCTAssertNotEqual(deliberateScrollY, topY)

      sidebar.select(appId: "app-0")
      XCTAssertEqual(appsScroll.contentView.bounds.origin.y, deliberateScrollY)

      sidebar.select(appId: "app-1")
      XCTAssertEqual(appsScroll.contentView.bounds.origin.y, topY)
    }
  }

  func testChatEmptyStateStartsAtTopAndClearsStaleScroll() throws {
    let html = try String(
      contentsOf: repoRoot().appendingPathComponent("apps/chat/index.html"),
      encoding: .utf8
    )
    XCTAssertTrue(
      html.contains(
        """
              #empty {
                opacity: 0.55;
                text-align: center;
                align-self: stretch;
                margin: 0;
        """))
    XCTAssertTrue(
      html.contains(
        """
                  emptyEl.hidden = false;
                  logEl.scrollTop = 0;
                  return;
        """))
  }

  func testAppSidebarHostsGenericSelectableSection() throws {
    try runOnMainThread {
      let sidebar = AppSidebarView(frame: NSRect(x: 0, y: 0, width: 232, height: 680))
      var selectedItem: String?
      var created = false
      sidebar.onSectionItemSelect = { selectedItem = $0 }
      sidebar.onSectionCreate = { created = true }
      sidebar.setAppSection(
        AppSidebarSection(
          title: "Documents",
          items: [
            AppSidebarSectionItem(
              id: "first", title: "First", subtitle: "2 items", systemImage: "doc"),
            AppSidebarSectionItem(
              id: "second", title: "Second", subtitle: nil, systemImage: nil),
          ],
          selectedItemId: "second",
          createLabel: "New document"
        ))
      sidebar.layoutSubtreeIfNeeded()

      let rows = subviews(ofType: AppSidebarItemButton.self, in: sidebar)
      XCTAssertEqual(rows.map(\.title), ["First", "Second"])
      XCTAssertEqual(rows.map(\.isSelected), [false, true])
      XCTAssertNotNil(rows[0].image)
      XCTAssertNil(rows[1].image)
      rows[0].performClick(nil)
      XCTAssertEqual(selectedItem, "first")

      let create = try XCTUnwrap(
        subviews(ofType: NSButton.self, in: sidebar).first {
          $0.accessibilityLabel() == "New document"
        })
      create.performClick(nil)
      XCTAssertTrue(created)

      let split = try XCTUnwrap(subviews(ofType: NSSplitView.self, in: sidebar).first)
      let detailHeader = try XCTUnwrap(
        subviews(ofType: NSButton.self, in: sidebar).first { $0.title == "Documents" })
      detailHeader.performClick(nil)
      sidebar.layoutSubtreeIfNeeded()
      XCTAssertLessThanOrEqual(split.subviews[1].frame.height, 40)
      XCTAssertGreaterThanOrEqual(split.subviews[0].frame.height, split.bounds.height - 40)

      let appsHeader = try XCTUnwrap(
        subviews(ofType: NSButton.self, in: sidebar).first { $0.title == "Apps" })
      appsHeader.performClick(nil)
      appsHeader.performClick(nil)
      sidebar.layoutSubtreeIfNeeded()
      XCTAssertLessThanOrEqual(split.subviews[1].frame.height, 40)
      XCTAssertGreaterThanOrEqual(split.subviews[0].frame.height, split.bounds.height - 40)

      sidebar.setCollapsed(true)
      sidebar.setCollapsed(false)
      sidebar.layoutSubtreeIfNeeded()
      XCTAssertFalse(detailHeader.isHidden)
      XCTAssertLessThanOrEqual(split.subviews[1].frame.height, 40)

      sidebar.setAppSection(nil)
      XCTAssertTrue(subviews(ofType: AppSidebarItemButton.self, in: sidebar).isEmpty)
    }
  }

  func testPremiumCatalogParsesPublicMarketplaceResponse() throws {
    let json = """
      {
        "ok": true,
        "result": {
          "apps": [
            {"id": "premium-todo", "name": "Premium Todo", "publisher": "Terrane Premium", "icon": "checklist", "serverRequired": true},
            {"id": "hold dear", "name": "Hold Dear", "publisher": "Hold Dear Studio"}
          ]
        }
      }
      """

    let apps = PremiumCatalog.parse(try XCTUnwrap(json.data(using: .utf8)))
    XCTAssertEqual(apps.map(\.id), ["premium-todo", "hold dear"])
    XCTAssertEqual(apps[0].name, "Premium Todo")
    XCTAssertEqual(apps[0].publisher, "Terrane Premium")
    XCTAssertEqual(apps[0].icon, "checklist")
    XCTAssertEqual(apps[1].publisher, "Hold Dear Studio")
    XCTAssertEqual(apps[1].dashboardFragment, "hold%20dear")
  }

  func testAppBuilderWaitsForExplicitBuildClick() throws {
    let repoRoot = repoRoot()
    let appSource = try String(
      contentsOf: repoRoot.appendingPathComponent("apps/app-builder/app.js"),
      encoding: .utf8
    )
    let html = try String(
      contentsOf: repoRoot.appendingPathComponent("apps/app-builder/index.html"),
      encoding: .utf8
    )

    XCTAssertTrue(appSource.contains(#"addEventListener("click", generate)"#), appSource)
    XCTAssertTrue(appSource.contains("builderGenerate"), appSource)
    XCTAssertTrue(appSource.contains(#"harness: harnessEl.value || "codex""#), appSource)
    XCTAssertFalse(appSource.contains("\n  generate();"), appSource)
    XCTAssertTrue(html.contains(#"id="generate""#), html)
    XCTAssertTrue(html.contains(#"id="harness""#), html)
    XCTAssertTrue(html.contains(#"value="claude-code""#), html)
    XCTAssertTrue(html.contains(#"value="opencode""#), html)
    XCTAssertTrue(html.contains(#"<span id="status" data-i18n="app-builder.ready">Ready</span>"#), html)
  }

  func testPhotoboothBundleUsesCameraOnlyAndIsServedByNativeHost() throws {
    let fixture = try AppFixture(appId: "photobooth")
    defer { fixture.cleanUp() }

    let apps = AppCatalog.discover(home: fixture.home)
    let app = try XCTUnwrap(apps.first { $0.id == "photobooth" })
    XCTAssertEqual(app.name, "Photobooth")
    XCTAssertEqual(app.uiURL.lastPathComponent, "index.html")
    XCTAssertEqual(app.iconPath, "icon.svg")
    XCTAssertEqual(app.browserPermissions, ["camera"])

    let html = try String(contentsOf: app.uiURL, encoding: .utf8)
    XCTAssertTrue(html.contains("navigator.mediaDevices.getUserMedia"), html)
    XCTAssertTrue(html.contains("function hasNativeCapture()"), html)
    XCTAssertTrue(html.contains("function hasNativeStream()"), html)
    XCTAssertTrue(html.contains(#""terrane:cameraFrame""#), html)
    XCTAssertTrue(html.contains("window.terrane.capturePhoto"), html)
    XCTAssertTrue(html.contains("window.terrane.startCameraStream"), html)
    XCTAssertTrue(html.contains("hasNativeCapture() && !navigator.mediaDevices"), html)
    XCTAssertTrue(html.contains("audio: false"), html)
    XCTAssertTrue(html.contains(#"toDataURL("image/png")"#), html)
    XCTAssertTrue(html.contains(#"download="photobooth.png""#), html)
    XCTAssertTrue(html.contains("function clearPhoto()"), html)
    XCTAssertTrue(html.contains("function hasLiveStream()"), html)
    XCTAssertTrue(html.contains("URL.createObjectURL"), html)
    XCTAssertTrue(html.contains("new Blob"), html)
    XCTAssertTrue(html.contains("URL.revokeObjectURL"), html)
    XCTAssertTrue(html.contains("video.srcObject = null"), html)
    XCTAssertTrue(html.contains("setTimeout(function ()"), html)
    XCTAssertTrue(html.contains("Camera request timed out. Try Start again."), html)
    XCTAssertTrue(html.contains(#"downloadLink.setAttribute("aria-disabled", "true")"#), html)

    let frameAsset = AppAssetStore.asset(apps: [app], appId: "photobooth", relPath: "")
    guard case .success(let frame) = frameAsset else {
      XCTFail("Photobooth frame should be served by terrane-app scheme")
      return
    }
    XCTAssertEqual(frame.contentType, "text/html; charset=utf-8")
    XCTAssertTrue(
      String(data: frame.data, encoding: .utf8)?.contains("Camera preview") == true)
  }

  func testMacHostDeclaresCameraUsageAndGrantsWebKitCameraCapture() throws {
    let root = repoRoot()
    let project = try String(
      contentsOf: root.appendingPathComponent("host/macos/project.yml"),
      encoding: .utf8
    )
    XCTAssertTrue(project.contains("NSCameraUsageDescription"), project)
    XCTAssertTrue(project.contains("NSMicrophoneUsageDescription"), project)
    XCTAssertTrue(project.contains("AVFoundation.framework"), project)
    XCTAssertTrue(
      project.contains("CODE_SIGN_ENTITLEMENTS: Sources/TerraneHost.entitlements"), project)

    let entitlements = try String(
      contentsOf: root.appendingPathComponent(
        "host/macos/Sources/TerraneHost.entitlements"),
      encoding: .utf8
    )
    XCTAssertTrue(entitlements.contains("com.apple.security.device.camera"), entitlements)
    XCTAssertTrue(entitlements.contains("com.apple.security.device.audio-input"), entitlements)

    let appDelegate = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/AppDelegate.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(appDelegate.contains("WKUIDelegate"), appDelegate)
    XCTAssertTrue(appDelegate.contains("WKNavigationDelegate"), appDelegate)
    XCTAssertTrue(appDelegate.contains("WKDownloadDelegate"), appDelegate)
    XCTAssertTrue(appDelegate.contains("webView.navigationDelegate = self"), appDelegate)
    XCTAssertTrue(appDelegate.contains("LoopbackAppHost"), appDelegate)
    XCTAssertTrue(appDelegate.contains("shouldUseLoopbackFrame"), appDelegate)
    XCTAssertTrue(appDelegate.contains("loopbackHost?.frameURL(for: app)"), appDelegate)
    XCTAssertTrue(appDelegate.contains("requestMediaCapturePermissionFor"), appDelegate)
    XCTAssertTrue(appDelegate.contains("registerSecureSchemes"), appDelegate)
    XCTAssertTrue(appDelegate.contains("_registerURLSchemeAsSecure:"), appDelegate)
    XCTAssertTrue(
      appDelegate.contains("[AppSchemeHandler.scheme, PreviewSchemeHandler.scheme]"), appDelegate)
    XCTAssertTrue(appDelegate.contains("selectedApp?.browserPermissions"), appDelegate)
    XCTAssertTrue(appDelegate.contains(#"permissions.contains("camera") ? .grant : .deny"#), appDelegate)
    XCTAssertTrue(appDelegate.contains(#"permissions.contains("microphone") ? .grant : .deny"#), appDelegate)
    XCTAssertTrue(appDelegate.contains("case .cameraAndMicrophone:"), appDelegate)
    XCTAssertTrue(appDelegate.contains("decisionHandler(.deny)"), appDelegate)
    XCTAssertTrue(appDelegate.contains("onCameraCapturePhoto"), appDelegate)
    XCTAssertTrue(appDelegate.contains("onCameraStreamStart"), appDelegate)
    XCTAssertTrue(appDelegate.contains("pushCameraFrame"), appDelegate)
    XCTAssertTrue(appDelegate.contains("NativeCameraCaptureController.present"), appDelegate)
    XCTAssertTrue(appDelegate.contains("navigationAction.shouldPerformDownload"), appDelegate)
    XCTAssertTrue(appDelegate.contains("decisionHandler(.download)"), appDelegate)
    XCTAssertTrue(appDelegate.contains("download.delegate = self"), appDelegate)
    XCTAssertTrue(appDelegate.contains("NSSavePanel"), appDelegate)

    let bridge = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/TerraneBridge.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(bridge.contains("browserPermissions = Set(app.browserPermissions)"), bridge)
    XCTAssertTrue(bridge.contains(#""camera:capturePhoto""#), bridge)
    XCTAssertTrue(bridge.contains(#""camera:startStream""#), bridge)
    XCTAssertTrue(bridge.contains("startCameraStream: function"), bridge)
    XCTAssertTrue(bridge.contains("browserPermissions.contains(\"camera\")"), bridge)
    XCTAssertTrue(bridge.contains("capturePhoto: function"), bridge)

    let nativeCapture = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/NativeCameraCapture.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(nativeCapture.contains("AVCaptureSession"), nativeCapture)
    XCTAssertTrue(nativeCapture.contains("AVCaptureVideoDataOutput"), nativeCapture)
    XCTAssertTrue(nativeCapture.contains("Waiting for camera frame"), nativeCapture)
    XCTAssertTrue(nativeCapture.contains("bestVideoDevice"), nativeCapture)
    XCTAssertTrue(nativeCapture.contains("!lhs.isSuspended && rhs.isSuspended"), nativeCapture)
    XCTAssertTrue(nativeCapture.contains(#"mime: "image/png""#), nativeCapture)

    let nativeStream = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/NativeCameraFrameStreamer.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(nativeStream.contains("AVCaptureVideoDataOutput"), nativeStream)
    XCTAssertTrue(nativeStream.contains(#"mime: "image/jpeg""#), nativeStream)
    XCTAssertTrue(nativeStream.contains("0.12"), nativeStream)

    let loopbackHost = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/LoopbackAppHost.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(loopbackHost.contains(#""--addr", "127.0.0.1:0""#), loopbackHost)
    XCTAssertTrue(loopbackHost.contains(#""--apps", appsDirectory.path"#), loopbackHost)
    XCTAssertTrue(loopbackHost.contains("terrane-loopback-\\(ProcessInfo.processInfo.processIdentifier)"), loopbackHost)
    XCTAssertTrue(loopbackHost.contains("http://localhost:"), loopbackHost)
    XCTAssertTrue(loopbackHost.contains("/apps/\\(app.id)/__terrane/frame/"), loopbackHost)
    XCTAssertTrue(project.contains("terrane-host-web"), project)
  }

  func testMacHostPromptsForTerranePermissionGrantsAndRetries() throws {
    let root = repoRoot()
    let bridge = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/TerraneBridge.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(bridge.contains("onPermissionRequired"), bridge)
    XCTAssertTrue(bridge.contains("PermissionRequiredPrompt.parse"), bridge)
    XCTAssertTrue(bridge.contains("self.invokeSelectedApp(verb: verb, args: args)"), bridge)

    let appDelegate = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/AppDelegate.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(appDelegate.contains("Allow \\(prompt.appName) to access resources?"), appDelegate)
    XCTAssertTrue(appDelegate.contains("bridge.grant(app: prompt.appId, namespace: namespace)"), appDelegate)
  }

  func testSourceSyntaxHighlighterColorsCodeTokens() throws {
    try runOnMainThread {
      let textView = NSTextView()
      textView.string = """
        import React from "react";
        const value = 42;
        // comment
        """

      SourceSyntaxHighlighter.apply(to: textView, fileExtension: "tsx")

      assertForegroundColor(in: textView, matching: "import", equals: .systemPurple)
      assertForegroundColor(in: textView, matching: "\"react\"", equals: .systemGreen)
      assertForegroundColor(in: textView, matching: "42", equals: .systemOrange)
      assertForegroundColor(in: textView, matching: "// comment", equals: .secondaryLabelColor)
    }
  }
}

private struct BmiFixture {
  private let fixture: AppFixture

  var home: URL {
    fixture.home
  }

  init() throws {
    fixture = try AppFixture(appId: "bmi-calculator")
  }

  func cleanUp() {
    fixture.cleanUp()
  }
}

private struct AppFixture {
  let home: URL

  init(appId: String) throws {
    let fm = FileManager.default
    home = fm.temporaryDirectory.appendingPathComponent(
      "terrane-\(appId)-e2e-\(UUID().uuidString)",
      isDirectory: true
    )
    try fm.createDirectory(at: home.appendingPathComponent("apps"), withIntermediateDirectories: true)

    let source = repoRoot().appendingPathComponent("apps/\(appId)", isDirectory: true)
    let destination = home.appendingPathComponent("apps/\(appId)", isDirectory: true)
    try Self.copyDirectory(from: source, to: destination)
  }

  func cleanUp() {
    try? FileManager.default.removeItem(at: home)
  }

  private static func copyDirectory(from source: URL, to destination: URL) throws {
    let fm = FileManager.default
    try fm.createDirectory(at: destination, withIntermediateDirectories: true)
    guard let enumerator = fm.enumerator(at: source, includingPropertiesForKeys: [.isDirectoryKey])
    else {
      throw CocoaError(.fileReadUnknown)
    }

    for case let item as URL in enumerator {
      let relativePath = String(item.path.dropFirst(source.path.count + 1))
      let target = destination.appendingPathComponent(relativePath)
      let values = try item.resourceValues(forKeys: [.isDirectoryKey])
      if values.isDirectory == true {
        try fm.createDirectory(at: target, withIntermediateDirectories: true)
      } else {
        try fm.createDirectory(
          at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        try fm.copyItem(at: item, to: target)
      }
    }
  }
}

private func repoRoot() -> URL {
  URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .standardizedFileURL
}

private func runOnMainThread<T>(_ body: () throws -> T) throws -> T {
  if Thread.isMainThread {
    return try body()
  }

  var result: Result<T, Error>!
  DispatchQueue.main.sync {
    result = Result { try body() }
  }
  return try result.get()
}

private func firstSubview<T: NSView>(ofType type: T.Type, in view: NSView) -> T? {
  subviews(ofType: type, in: view).first
}

private func subviews<T: NSView>(ofType type: T.Type, in view: NSView) -> [T] {
  var matches: [T] = []
  for subview in view.subviews {
    if let match = subview as? T {
      matches.append(match)
    }
    matches.append(contentsOf: subviews(ofType: type, in: subview))
  }
  return matches
}

private func assertForegroundColor(
  in textView: NSTextView,
  matching needle: String,
  equals expected: NSColor,
  file: StaticString = #filePath,
  line: UInt = #line
) {
  let range = (textView.string as NSString).range(of: needle)
  XCTAssertNotEqual(range.location, NSNotFound, "Missing \(needle)", file: file, line: line)
  guard range.location != NSNotFound else { return }
  let actual =
    textView.textStorage?.attribute(
      .foregroundColor,
      at: range.location,
      effectiveRange: nil
    ) as? NSColor
  XCTAssertTrue(
    actual?.isEqual(expected) == true,
    "\(needle) color was \(String(describing: actual)), expected \(expected)",
    file: file,
    line: line
  )
}
