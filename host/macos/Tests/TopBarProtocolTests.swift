import Foundation
import WebKit
import XCTest

/// The macOS host's top-bar document/theme protocol: the injected shim exposes
/// the same window.terrane surface as the web host, and the native helpers that
/// sanitize and marshal document names hold the line.
final class TopBarProtocolTests: XCTestCase {
  private func tempHome() throws -> URL {
    let home = FileManager.default.temporaryDirectory.appendingPathComponent(
      "terrane-topbar-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    return home
  }

  func testInjectedShimExposesDocumentAndThemeApi() throws {
    let home = try tempHome()
    defer { try? FileManager.default.removeItem(at: home) }

    let bridge = try XCTUnwrap(TerraneBridge(home: home))
    defer { bridge.close() }

    let userContent = WKUserContentController()
    bridge.install(into: userContent)
    let shim = try XCTUnwrap(userContent.userScripts.first).source

    for needle in [
      "getDocument:",
      "setDocument:",
      "onDocument:",
      "getTheme:",
      "onTheme:",
      "window.__terrane_apply",
      "kind: \"document:set\"",
    ] {
      XCTAssertTrue(shim.contains(needle), "shim missing \(needle)")
    }
  }

  func testSanitizeDocNameStripsControlAndBidiAndCaps() {
    XCTAssertEqual(TerraneBridge.sanitizeDocName("  hello   world  "), "hello world")
    // U+202E right-to-left override must be stripped.
    XCTAssertEqual(TerraneBridge.sanitizeDocName("safe\u{202e}evil"), "safeevil")
    // Zero-width + control chars stripped.
    XCTAssertEqual(TerraneBridge.sanitizeDocName("a\u{200b}b\u{0007}c"), "abc")
    XCTAssertEqual(TerraneBridge.sanitizeDocName("   "), "Untitled")
    XCTAssertEqual(TerraneBridge.sanitizeDocName(""), "Untitled")
    XCTAssertEqual(TerraneBridge.sanitizeDocName(String(repeating: "x", count: 200)).count, 120)
  }

  func testApplyStateJsEscapesAndOmitsMissingFields() {
    let js = TerraneBridge.applyStateJS(document: "a\"</script>", theme: "dark")
    XCTAssertTrue(js.contains("window.__terrane_apply"))
    XCTAssertTrue(js.contains("theme:\"dark\""))
    // The document must be escaped so it cannot break out of the JS string.
    XCTAssertFalse(js.contains("</script>"))
    XCTAssertTrue(js.contains("\\u003c/script>"))
    XCTAssertTrue(js.contains("\\\""))

    let docOnly = TerraneBridge.applyStateJS(document: "doc", theme: nil)
    XCTAssertTrue(docOnly.contains("document:\"doc\""))
    XCTAssertFalse(docOnly.contains("theme:"))
  }

  func testJsonStringLiteralEscapesDangerousScalars() {
    XCTAssertEqual(TerraneBridge.jsonStringLiteral("plain"), "\"plain\"")
    XCTAssertEqual(TerraneBridge.jsonStringLiteral("a\\b"), "\"a\\\\b\"")
    XCTAssertEqual(TerraneBridge.jsonStringLiteral("<x>"), "\"\\u003cx>\"")
    XCTAssertEqual(TerraneBridge.jsonStringLiteral("line\nbreak"), "\"line\\nbreak\"")
  }
}
