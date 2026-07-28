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

  func testProviderSheetPreservesLocalFirstBoundaryUntilExplicitlyPresented() {
    let parent = NSWindow()
    let sheet = PremiumSignInSheetController(parent: parent) { _ in }
    let labels = descendants(of: sheet.panelForTesting.contentView)
      .compactMap { ($0 as? NSTextField)?.stringValue }

    XCTAssertNil(sheet.panelForTesting.sheetParent)
    XCTAssertTrue(labels.contains("Enable Terrane Sync"))
    XCTAssertTrue(
      labels.contains {
        $0.contains("Sign in only to enable sync")
          && $0.contains("fully usable locally and offline without an account")
      }
    )
  }

  func testStartupConfigurationCannotPresentProviderSheet() throws {
    let source = try String(
      contentsOf: repoRoot().appendingPathComponent("host/macos/Sources/AppDelegate.swift"),
      encoding: .utf8
    )
    let configure = try XCTUnwrap(
      source.range(
        of: "private func configurePremiumAccount()",
        range: source.startIndex..<source.endIndex
      )
    )
    let update = try XCTUnwrap(
      source.range(
        of: "private func updatePremiumAccountControl",
        range: configure.upperBound..<source.endIndex
      )
    )
    let accountClick = try XCTUnwrap(
      source.range(
        of: "@objc private func accountButtonClicked",
        range: update.upperBound..<source.endIndex
      )
    )
    let accountMenu = try XCTUnwrap(
      source.range(
        of: "private func presentPremiumAccountMenu",
        range: accountClick.upperBound..<source.endIndex
      )
    )

    XCTAssertFalse(source[configure.lowerBound..<update.lowerBound].contains(".present()"))
    XCTAssertTrue(source[accountClick.lowerBound..<accountMenu.lowerBound].contains(".present()"))
    XCTAssertTrue(source.contains("accountButton.title = \"Enable Sync\""))
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
