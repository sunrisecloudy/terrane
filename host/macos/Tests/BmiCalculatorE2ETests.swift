import Foundation
import WebKit
import XCTest

final class BmiCalculatorE2ETests: XCTestCase {
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
        uiURL: base.appendingPathComponent("todo/index.html")
      )
      let paint = TerraneApp(
        id: "pixel-paint",
        name: "Pixel Paint",
        directory: base.appendingPathComponent("pixel-paint"),
        uiURL: base.appendingPathComponent("pixel-paint/index.html")
      )

      let sidebar = AppSidebarView(frame: NSRect(x: 0, y: 0, width: 224, height: 640))
      var selected: TerraneApp?
      sidebar.onSelect = { selected = $0 }
      sidebar.render(apps: [paint, todo], selectedAppId: "todo")
      sidebar.layoutSubtreeIfNeeded()

      let buttons = subviews(ofType: AppSidebarButton.self, in: sidebar)
      XCTAssertEqual(buttons.map(\.title), ["Pixel Paint", "Todo"])
      XCTAssertEqual(buttons.map(\.toolTip), ["pixel-paint", "todo"])
      XCTAssertEqual(buttons.map(\.isSelected), [false, true])
      XCTAssertTrue(buttons.allSatisfy { $0.image != nil })

      sidebar.selectApp(at: 0)
      XCTAssertEqual(selected?.id, "pixel-paint")

      sidebar.select(appId: "pixel-paint")
      XCTAssertEqual(buttons.map(\.isSelected), [true, false])

      sidebar.setCollapsed(true)
      XCTAssertEqual(buttons.map(\.title), ["", ""])
      XCTAssertEqual(buttons.map(\.toolTip), ["Pixel Paint", "Todo"])

      sidebar.setCollapsed(false)
      XCTAssertEqual(buttons.map(\.title), ["Pixel Paint", "Todo"])
    }
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
    XCTAssertTrue(html.contains(#"<span id="status">Ready</span>"#), html)
  }

  func testPhotoboothBundleUsesCameraOnlyAndIsServedByNativeHost() throws {
    let fixture = try AppFixture(appId: "photobooth")
    defer { fixture.cleanUp() }

    let apps = AppCatalog.discover(home: fixture.home)
    let app = try XCTUnwrap(apps.first { $0.id == "photobooth" })
    XCTAssertEqual(app.name, "Photobooth")
    XCTAssertEqual(app.uiURL.lastPathComponent, "index.html")

    let html = try String(contentsOf: app.uiURL, encoding: .utf8)
    XCTAssertTrue(html.contains("navigator.mediaDevices.getUserMedia"), html)
    XCTAssertTrue(html.contains("audio: false"), html)
    XCTAssertTrue(html.contains(#"toDataURL("image/png")"#), html)
    XCTAssertTrue(html.contains(#"download="photobooth.png""#), html)

    let frameAsset = AppAssetStore.asset(apps: [app], appId: "photobooth", relPath: "")
    guard case .success(let frame) = frameAsset else {
      XCTFail("Photobooth frame should be served by terrane-app scheme")
      return
    }
    XCTAssertEqual(frame.contentType, "text/html; charset=utf-8")
    XCTAssertTrue(
      String(data: frame.data, encoding: .utf8)?.contains("Camera preview") == true)
  }

  func testMacHostDeclaresCameraUsageAndPromptsForWebKitCameraCapture() throws {
    let root = repoRoot()
    let project = try String(
      contentsOf: root.appendingPathComponent("host/macos/project.yml"),
      encoding: .utf8
    )
    XCTAssertTrue(project.contains("NSCameraUsageDescription"), project)

    let appDelegate = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/AppDelegate.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(appDelegate.contains("WKUIDelegate"), appDelegate)
    XCTAssertTrue(appDelegate.contains("requestMediaCapturePermissionFor"), appDelegate)
    XCTAssertTrue(appDelegate.contains("case .camera:"), appDelegate)
    XCTAssertTrue(appDelegate.contains("decisionHandler(.prompt)"), appDelegate)
    XCTAssertTrue(appDelegate.contains("case .microphone, .cameraAndMicrophone:"), appDelegate)
    XCTAssertTrue(appDelegate.contains("decisionHandler(.deny)"), appDelegate)
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
