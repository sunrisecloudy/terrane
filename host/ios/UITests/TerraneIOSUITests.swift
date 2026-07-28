import XCTest

final class TerraneIOSUITests: XCTestCase {
  func testLocalAppsRemainAvailableWithoutPremiumConfiguration() {
    let app = XCUIApplication()
    app.launch()

    XCTAssertTrue(app.navigationBars["Terrane"].waitForExistence(timeout: 10))
    XCTAssertFalse(app.staticTexts["No local apps"].exists)

    app.tabBars.buttons["Account"].tap()
    XCTAssertTrue(app.staticTexts["Premium is not configured"].waitForExistence(timeout: 5))

    app.tabBars.buttons["Apps"].tap()
    XCTAssertTrue(app.navigationBars["Terrane"].waitForExistence(timeout: 5))
  }
}
