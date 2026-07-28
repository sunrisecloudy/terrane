import AppKit
import AuthenticationServices
import XCTest

final class PremiumNativeAuthTests: XCTestCase {
  func testProviderSheetUsesNativeAppleAndHostedGoogleControls() {
    let parent = NSWindow()
    let sheet = PremiumSignInSheetController(parent: parent) { _ in }
    let views = descendants(of: sheet.panelForTesting.contentView)

    XCTAssertTrue(views.contains { $0 is ASAuthorizationAppleIDButton })
    XCTAssertTrue(views.contains { String(describing: type(of: $0)).contains("NSHostingView") })
    XCTAssertFalse(
      views.contains { String(describing: type(of: $0)).contains("WKWebView") },
      "Authentication controls must remain outside app-content WKWebViews."
    )
  }

  func testNativeCoordinatorUsesSharedSessionBoundaryAndDropsGoogleSDKSession() throws {
    let source = try String(
      contentsOf: repoRoot()
        .appendingPathComponent("host/macos/Sources/PremiumNativeAuth.swift"),
      encoding: .utf8
    )

    XCTAssertTrue(source.contains("PremiumSessionClient"))
    XCTAssertTrue(source.contains("exchangeAppleCredential"))
    XCTAssertTrue(source.contains("exchangeGoogleCredential"))
    XCTAssertTrue(source.contains("linkAppleCredential"))
    XCTAssertTrue(source.contains("linkGoogleCredential"))
    XCTAssertTrue(source.contains("GIDSignIn.sharedInstance.signOut()"))
    XCTAssertFalse(source.contains("WKWebView"))
    XCTAssertFalse(source.contains("TerraneBridge"))
  }

  private func descendants(of view: NSView?) -> [NSView] {
    guard let view else { return [] }
    return [view] + view.subviews.flatMap { descendants(of: $0) }
  }

  private func repoRoot() -> URL {
    URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
  }
}
