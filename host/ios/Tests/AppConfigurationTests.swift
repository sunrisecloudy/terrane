import XCTest
@testable import TerraneIOS

final class AppConfigurationTests: XCTestCase {
  func testUnconfiguredBuildKeepsPremiumOptional() {
    let bundle = Bundle(for: Self.self)
    let configuration = AppConfiguration(bundle: bundle)
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
}
