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
    XCTAssertEqual(
      project.components(separatedBy: "ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon").count - 1,
      2,
      "bootstrap and runtime must use the canonical AppIcon asset"
    )
    XCTAssertEqual(
      project.components(separatedBy: "path: SharedAssets/TerraneAssets.xcassets").count - 1,
      2,
      "bootstrap and runtime must embed the same icon catalog"
    )
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath:
          root.appendingPathComponent(
            "host/macos/SharedAssets/TerraneAssets.xcassets/AppIcon.appiconset/Contents.json"
          ).path
      )
    )
    let iconContentsURL = root.appendingPathComponent(
      "host/macos/SharedAssets/TerraneAssets.xcassets/AppIcon.appiconset/Contents.json")
    let iconContents = try String(contentsOf: iconContentsURL, encoding: .utf8)
    let iconFilenames = iconContents.matches(of: /"filename"\s*:\s*"([^"]+)"/)
      .map { String($0.1) }
    XCTAssertEqual(iconFilenames.count, 10, "macOS AppIcon must contain every 1x and 2x size")
    for filename in iconFilenames {
      XCTAssertTrue(
        FileManager.default.fileExists(
          atPath: iconContentsURL.deletingLastPathComponent()
            .appendingPathComponent(filename).path),
        "missing generated icon rendition: \(filename)"
      )
    }
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
        "https://github.com/sunrisecloudy/terrane/releases/latest/download/terrane-bootstrap-manifest.json"
      ),
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
