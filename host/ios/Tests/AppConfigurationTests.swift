import TerranePremiumSession
import UIKit
import XCTest

@testable import TerraneIOS

final class AppConfigurationTests: XCTestCase {
  func testUnconfiguredBuildKeepsPremiumOptional() {
    let bundle = Bundle(for: Self.self)
    let configuration = AppConfiguration(bundle: bundle, environment: [:])
    XCTAssertNil(configuration.premiumBaseURL)
    XCTAssertNil(configuration.googleClientID)
  }

  func testBridgeSurfaceDoesNotContainPremiumCredentials() {
    let script = TerraneAppWebView.bridgeScript
    XCTAssertTrue(script.contains("terrane"))
    XCTAssertFalse(script.localizedCaseInsensitiveContains("token"))
    XCTAssertFalse(script.localizedCaseInsensitiveContains("premium"))
    XCTAssertFalse(script.localizedCaseInsensitiveContains("account"))
  }

  func testSimulatorUsesEmbeddedHostRuntime() {
    #if targetEnvironment(simulator)
      let runtime = TerraneRuntimeFactory.make()
      XCTAssertEqual(runtime.availability, .embedded)
      runtime.close()
    #endif
  }

  func testBundledAppRegistrationArgumentsKeepRuntimeAndInterfaces() {
    let app = TerraneApp(
      id: "health",
      name: "Health",
      icon: "health",
      directory: URL(fileURLWithPath: "/bundle/apps/health"),
      uiPath: "dist/index.html",
      runtime: "js",
      interfaces: ["items"]
    )
    XCTAssertEqual(
      app.registrationArguments,
      [
        "health",
        "Health",
        "--source",
        "/bundle/apps/health",
        "--runtime",
        "js",
        "--interfaces",
        "items",
        "--refresh-source",
      ]
    )
  }

  func testPermissionRequestParserAcceptsOnlyTheSelectedApp() {
    XCTAssertEqual(
      IOSPermissionRequestParser.parse(
        error:
          "permission required for app health: grant blob,kv,model; open http://127.0.0.1:8780",
        appID: "health"
      ),
      ["blob", "kv", "model"]
    )
    XCTAssertNil(
      IOSPermissionRequestParser.parse(
        error: "permission required for app spending: grant blob,kv,model",
        appID: "health"
      )
    )
  }

  func testPinnedAppPreferencesPersistAndDeduplicate() {
    let suiteName = "AppConfigurationTests.pins.\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suiteName) else {
      return XCTFail("Could not create isolated defaults")
    }
    defer {
      defaults.removePersistentDomain(forName: suiteName)
    }

    defaults.set(["health", "health", "  ", "calendar"], forKey: AppPinPreferences.storageKey)
    XCTAssertEqual(AppPinPreferences.load(from: defaults), ["health", "calendar"])

    AppPinPreferences.save(["spending", "health"], to: defaults)
    XCTAssertEqual(
      defaults.stringArray(forKey: AppPinPreferences.storageKey), ["health", "spending"])
  }

  func testEveryBundledAppHasAnAvailableDedicatedNativeIcon() {
    let apps = BundledAppCatalog.load(bundle: Bundle(for: TerraneIOSModel.self))
    let appIDs = Set(apps.map(\.id))

    XCTAssertFalse(appIDs.isEmpty)
    XCTAssertTrue(
      appIDs.isSubset(of: Set(NativeAppIconCatalog.symbolByAppID.keys)),
      "Every app visible in the iOS catalog must have a dedicated icon"
    )
    for app in apps {
      let symbol = NativeAppIconCatalog.systemName(for: app)
      XCTAssertNotNil(UIImage(systemName: symbol), "\(app.id) has unavailable symbol \(symbol)")
    }
    XCTAssertEqual(
      Set(NativeAppIconCatalog.symbolByAppID.values).count,
      NativeAppIconCatalog.symbolByAppID.count,
      "Every app should have a visually distinct icon"
    )
  }

  func testGoogleNativeSignInForwardsTheRawChallengeNonce() throws {
    let challenge = PremiumAuthenticationChallenge(
      challengeId: "google-challenge",
      provider: .google,
      nonce: "raw-google-nonce",
      nonceSha256: "apple-only-hash"
    )

    XCTAssertEqual(
      try GoogleNativeChallengeNonce.require(from: challenge),
      "raw-google-nonce"
    )
    XCTAssertThrowsError(
      try GoogleNativeChallengeNonce.require(
        from: PremiumAuthenticationChallenge(
          challengeId: "missing-nonce",
          provider: .google
        )
      )
    )
  }
}
