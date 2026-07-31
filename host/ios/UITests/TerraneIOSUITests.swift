import XCTest

final class TerraneIOSUITests: XCTestCase {
  func testLocalAppsRemainAvailableWithoutPremiumConfiguration() {
    let app = XCUIApplication()
    app.launch()

    XCTAssertTrue(app.navigationBars["Terrane"].waitForExistence(timeout: 10))
    XCTAssertFalse(app.staticTexts["No local apps"].exists)

    app.tabBars.buttons["Account"].tap()
    let unconfigured = app.staticTexts["Premium is not configured"]
    let configured = app.staticTexts["Terrane Premium"]
    XCTAssertTrue(
      unconfigured.waitForExistence(timeout: 2) || configured.waitForExistence(timeout: 5),
      "The Account tab should explain either the optional unconfigured state or show Premium"
    )

    app.tabBars.buttons["Apps"].tap()
    XCTAssertTrue(app.navigationBars["Terrane"].waitForExistence(timeout: 5))
  }

  func testHealthCatalogsBackendAndRequestsResources() {
    let app = XCUIApplication()
    app.launch()

    let health = app.descendants(matching: .any).matching(identifier: "app.health").firstMatch
    XCTAssertTrue(health.waitForExistence(timeout: 10))
    health.tap()
    XCTAssertTrue(app.navigationBars["Health"].waitForExistence(timeout: 5))

    let permissionAlert = app.alerts.firstMatch
    if permissionAlert.waitForExistence(timeout: 10) {
      let allow = permissionAlert.buttons["Allow"]
      XCTAssertTrue(allow.exists)
      allow.tap()
    }

    XCTAssertFalse(app.staticTexts["no such app: health"].waitForExistence(timeout: 3))
  }

  func testVisibleAppsExposeDedicatedNativeIcons() {
    let app = XCUIApplication()
    app.launch()

    XCTAssertTrue(app.images["app.health.icon"].waitForExistence(timeout: 10))
    XCTAssertTrue(app.images["app.chat.icon"].waitForExistence(timeout: 3))
  }

  func testAppCanBePinnedAndUnpinned() {
    let app = XCUIApplication()
    app.launch()

    var health = app.descendants(matching: .any).matching(identifier: "app.health").firstMatch
    XCTAssertTrue(health.waitForExistence(timeout: 10))
    health.swipeRight()

    if app.buttons["Unpin"].waitForExistence(timeout: 2) {
      app.buttons["Unpin"].tap()
      health = app.descendants(matching: .any).matching(identifier: "app.health").firstMatch
      health.swipeRight()
    }

    let pin = app.buttons["Pin"]
    XCTAssertTrue(pin.waitForExistence(timeout: 3))
    pin.tap()
    XCTAssertTrue(
      app.descendants(matching: .any)
        .matching(identifier: "app.health.pinned")
        .firstMatch
        .waitForExistence(timeout: 3)
    )
    XCTAssertTrue(app.tabBars.buttons["Health"].waitForExistence(timeout: 3))

    app.terminate()
    app.launch()
    XCTAssertTrue(
      app.descendants(matching: .any)
        .matching(identifier: "app.health.pinned")
        .firstMatch
        .waitForExistence(timeout: 10)
    )
    XCTAssertTrue(app.tabBars.buttons["Health"].waitForExistence(timeout: 10))

    app.tabBars.buttons["Apps"].tap()
    health = app.descendants(matching: .any).matching(identifier: "app.health").firstMatch
    health.swipeRight()
    let unpin = app.buttons["Unpin"]
    XCTAssertTrue(unpin.waitForExistence(timeout: 3))
    unpin.tap()
    XCTAssertFalse(
      app.descendants(matching: .any)
        .matching(identifier: "app.health.pinned")
        .firstMatch
        .exists
    )
    XCTAssertFalse(app.tabBars.buttons["Health"].exists)
  }
}
