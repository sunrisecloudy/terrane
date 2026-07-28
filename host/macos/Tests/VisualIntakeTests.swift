import Foundation
import XCTest

final class VisualIntakeTests: XCTestCase {
  func testFoodEvidenceRecommendsHealth() {
    let decision = VisualIntakeRoutingDecision.decide(
      classifications: [("food", 0.87), ("table", 0.32)],
      recognizedLines: [],
      isScreenshot: false,
      faceCount: 0
    )

    XCTAssertEqual(decision.intents.first?.name, "food")
    XCTAssertEqual(decision.recommendedAppId, "health")
    XCTAssertEqual(decision.recommendedAppName, "Health")
    XCTAssertEqual(decision.sensitivity, "normal")
    XCTAssertTrue(decision.evidenceCodes.contains("vision.food"))
  }

  func testInvoiceEvidenceRecommendsInvoiceAndRequiresReview() {
    let decision = VisualIntakeRoutingDecision.decide(
      classifications: [("document", 0.7)],
      recognizedLines: [
        "Invoice number 1042",
        "Bill to Example Company",
        "Consulting services",
        "Amount due $1,250.00",
      ],
      isScreenshot: false,
      faceCount: 0
    )

    XCTAssertEqual(decision.intents.first?.name, "invoice")
    XCTAssertEqual(decision.recommendedAppId, "invoice")
    XCTAssertEqual(decision.sensitivity, "review-required")
    XCTAssertTrue(decision.evidenceCodes.contains("ocr.invoice-cues"))
  }

  func testScreenshotMetadataRecommendsSearchNotes() {
    let decision = VisualIntakeRoutingDecision.decide(
      classifications: [],
      recognizedLines: [],
      isScreenshot: true,
      faceCount: 0
    )

    XCTAssertEqual(decision.intents.first?.name, "screenshot")
    XCTAssertEqual(decision.recommendedAppId, "search-notes")
    XCTAssertTrue(decision.evidenceCodes.contains("photos.screenshot"))
  }

  func testFaceEvidenceRequiresReview() {
    let decision = VisualIntakeRoutingDecision.decide(
      classifications: [("portrait", 0.9)],
      recognizedLines: [],
      isScreenshot: false,
      faceCount: 1
    )

    XCTAssertEqual(decision.sensitivity, "review-required")
    XCTAssertTrue(decision.evidenceCodes.contains("vision.face-present"))
  }

  func testVisualIntakeBundleAndPrivacyBoundary() throws {
    let root = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
    let app = root.appendingPathComponent("apps/visual-intake")
    let manifest = try String(
      contentsOf: app.appendingPathComponent("manifest.json"), encoding: .utf8)
    let html = try String(
      contentsOf: app.appendingPathComponent("index.html"), encoding: .utf8)
    let bridge = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/TerraneBridge.swift"),
      encoding: .utf8)
    let project = try String(
      contentsOf: root.appendingPathComponent("host/macos/project.yml"), encoding: .utf8)
    let entitlements = try String(
      contentsOf: root.appendingPathComponent(
        "host/macos/Sources/TerraneHost.entitlements"),
      encoding: .utf8)

    XCTAssertTrue(manifest.contains(#""browser_permissions": ["photos"]"#), manifest)
    XCTAssertTrue(html.contains("Enable and watch new images"), html)
    XCTAssertTrue(html.contains("Analyze latest image"), html)
    XCTAssertTrue(html.contains("No model invoked"), html)
    XCTAssertTrue(html.contains("No route executed"), html)
    XCTAssertTrue(bridge.contains("startVisualIntake: function"), bridge)
    XCTAssertTrue(bridge.contains(#"appId == "visual-intake""#), bridge)
    XCTAssertTrue(bridge.contains(#"browserPermissions.contains("photos")"#), bridge)
    XCTAssertTrue(project.contains("NSPhotoLibraryUsageDescription"), project)
    XCTAssertTrue(project.contains("Photos.framework"), project)
    XCTAssertTrue(project.contains("Vision.framework"), project)
    XCTAssertTrue(
      entitlements.contains("com.apple.security.personal-information.photos-library"),
      entitlements)
  }
}
