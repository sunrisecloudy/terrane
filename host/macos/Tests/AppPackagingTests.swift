import Foundation
import XCTest

final class AppPackagingTests: XCTestCase {
  func testMacBuildEmbedsEveryCheckedInAppBundle() throws {
    let root = repositoryRoot()
    let appsDirectory = root.appendingPathComponent("apps")
    let project = try String(
      contentsOf: root.appendingPathComponent("host/macos/project.yml"),
      encoding: .utf8
    )
    let bridge = try String(
      contentsOf: root.appendingPathComponent("host/macos/Sources/TerraneBridge.swift"),
      encoding: .utf8
    )
    XCTAssertTrue(project.contains(#"name: "Package built-in apps""#), project)
    XCTAssertTrue(project.contains(#"APP_SOURCE="$SRCROOT/../../apps""#), project)
    XCTAssertTrue(
      project.contains(
        #"APP_DESTINATION="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/apps""#),
      project
    )
    XCTAssertTrue(project.contains("rsync -a --delete"), project)
    XCTAssertTrue(project.contains("DESTINATION_COUNT"), project)
    XCTAssertTrue(project.contains("-force_load"), project)
    XCTAssertTrue(
      project.contains("$(TERRANE_RUST_LIB_DIR)/libterrane_host.a"), project)
    XCTAssertTrue(
      project.contains("$(TERRANE_RUST_LIB_DIR)/libterrane_host_httplib.a"), project)
    XCTAssertTrue(
      project.contains(
        "TERRANE_RUST_LIB_DIR: $(SRCROOT)/../../target/macos-static/release"), project)
    XCTAssertTrue(project.contains(#"if [ "${TERRANE_SKIP_RUST_BUILD:-0}" = "1" ]"#), project)
    XCTAssertTrue(project.contains("outputFiles:"), project)
    XCTAssertTrue(project.contains(#"CMAKE_TOOLCHAIN_FILE="$SRCROOT/cmake/static-host.cmake""#))
    XCTAssertTrue(project.contains("llama-cpp-sys did not produce libcpp-httplib.a"), project)
    XCTAssertTrue(project.contains(#"DEAD_CODE_STRIPPING: "YES""#), project)
    XCTAssertFalse(project.contains("-lterrane_host"), project)
    XCTAssertTrue(bridge.contains(#""--refresh-source""#), bridge)
    let manifests = try FileManager.default.contentsOfDirectory(
      at: appsDirectory,
      includingPropertiesForKeys: [.isDirectoryKey],
      options: [.skipsHiddenFiles]
    ).filter { directory in
      (try? directory.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        && FileManager.default.fileExists(
          atPath: directory.appendingPathComponent("manifest.json").path)
    }
    XCTAssertEqual(manifests.count, 12, "all checked-in app bundles must be packaged")
  }

  func testBootstrapIsASeparateThinApplication() throws {
    let root = repositoryRoot()
    let project = try String(
      contentsOf: root.appendingPathComponent("host/macos/project.yml"),
      encoding: .utf8
    )
    let configuration = try String(
      contentsOf: root.appendingPathComponent(
        "host/macos/BootstrapSources/BootstrapConfiguration.swift"),
      encoding: .utf8
    )

    XCTAssertTrue(project.contains("TerraneBootstrap:"), project)
    XCTAssertTrue(project.contains("PRODUCT_BUNDLE_IDENTIFIER: com.terrane.bootstrap"), project)
    XCTAssertTrue(project.contains("EXECUTABLE_NAME: TerraneBootstrap"), project)
    XCTAssertTrue(project.contains("TerraneBootstrapTests:"), project)
    XCTAssertTrue(
      configuration.contains(
        "https://github.com/sunrisecloudy/terrane/releases/latest/download/terrane-bootstrap-manifest.json"),
      configuration
    )
  }

  private func repositoryRoot() -> URL {
    URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
  }
}
