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
      project.contains("$(SRCROOT)/../../target/macos-static/release/libterrane_host.a"), project)
    XCTAssertTrue(
      project.contains(
        "$(SRCROOT)/../../target/macos-static/release/libterrane_host_httplib.a"), project)
    XCTAssertTrue(project.contains("outputFiles:"), project)
    XCTAssertTrue(project.contains(#"CMAKE_TOOLCHAIN_FILE="$SRCROOT/cmake/static-host.cmake""#))
    XCTAssertTrue(project.contains("llama-cpp-sys did not produce libcpp-httplib.a"), project)
    XCTAssertTrue(project.contains(#"DEAD_CODE_STRIPPING: "YES""#), project)
    XCTAssertFalse(project.contains("-lterrane_host"), project)
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

  private func repositoryRoot() -> URL {
    URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
  }
}
