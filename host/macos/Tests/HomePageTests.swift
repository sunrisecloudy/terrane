import Foundation
import XCTest

final class HomePageTests: XCTestCase {
  private func app(id: String, name: String) -> TerraneApp {
    let dir = URL(fileURLWithPath: "/tmp/terrane-home-tests/\(id)")
    return TerraneApp(
      id: id,
      name: name,
      directory: dir,
      uiURL: dir.appendingPathComponent("index.html"),
      iconPath: "icon.svg",
      iconURL: dir.appendingPathComponent("icon.svg"),
      browserPermissions: []
    )
  }

  func testRendersSharedLandingPageWithInlineCatalogAndSchemeLinks() throws {
    let html = try XCTUnwrap(
      HomePage.render(apps: [app(id: "todo", name: "Todo"), app(id: "pix", name: "Pixel Paint")]),
      "the shared landing page should render over the C ABI"
    )

    XCTAssertTrue(html.contains("<h1>Terrane</h1>"), html)
    XCTAssertTrue(html.contains(#""appHref":"terrane-app://{id}/frame/""#), html)
    // The native catalog is inlined into the page config (no fetch on macOS).
    XCTAssertTrue(html.contains(#"\"id\":\"todo\""#), html)
    XCTAssertTrue(html.contains(#"\"name\":\"Pixel Paint\""#), html)
    XCTAssertTrue(html.contains(#"\"icon\":\"terrane-app://todo/asset/icon.svg\""#), html)
    XCTAssertTrue(html.contains(#"\"has_ui\":true"#), html)
    XCTAssertFalse(html.contains(#""catalogUrl":"#), html)
    XCTAssertFalse(html.contains(#""adminHref":"#), html)
  }

  func testHostileAppNamesCannotCloseTheConfigScript() throws {
    let html = try XCTUnwrap(
      HomePage.render(apps: [app(id: "x", name: "Evil </script><script>alert(1)</script>")])
    )
    XCTAssertFalse(html.contains("Evil </script>"), html)
    XCTAssertTrue(html.contains(#"Evil \u003c/script>"#), html)
  }

  func testEmptyCatalogStillRenders() throws {
    let html = try XCTUnwrap(HomePage.render(apps: []))
    XCTAssertTrue(html.contains("<h1>Terrane</h1>"), html)
    XCTAssertTrue(html.contains(#"\"apps\":[]"#), html)
  }

  func testAppIdForURLMatchesOnlyAppFrameRoots() {
    XCTAssertEqual(HomePage.appId(for: URL(string: "terrane-app://todo/frame/")!), "todo")
    XCTAssertEqual(HomePage.appId(for: URL(string: "terrane-app://todo/frame")!), "todo")
    XCTAssertEqual(
      HomePage.appId(for: URL(string: "terrane-app://a%20b/frame/")!), "a b",
      "percent-encoded ids should decode"
    )
    XCTAssertNil(HomePage.appId(for: URL(string: "terrane-app://todo/frame/assets/app.css")!))
    XCTAssertNil(HomePage.appId(for: URL(string: "terrane-preview://todo/frame/")!))
    XCTAssertNil(HomePage.appId(for: URL(string: "https://example.com/frame/")!))
  }
}
