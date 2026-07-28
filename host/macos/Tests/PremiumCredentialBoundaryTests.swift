import Foundation
import XCTest

final class PremiumCredentialBoundaryTests: XCTestCase {
  func testGeneratedAppBridgeHasNoPremiumCredentialSurface() throws {
    let testsDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    let bridgeURL =
      testsDirectory
      .deletingLastPathComponent()
      .appendingPathComponent("Sources/TerraneBridge.swift")
    let bridge = try String(contentsOf: bridgeURL, encoding: .utf8)

    XCTAssertFalse(bridge.contains("TerranePremiumSession"))
    XCTAssertFalse(bridge.contains("accessToken"))
    XCTAssertFalse(bridge.contains("refreshToken"))
    XCTAssertFalse(bridge.contains("Authorization"))
  }
}
