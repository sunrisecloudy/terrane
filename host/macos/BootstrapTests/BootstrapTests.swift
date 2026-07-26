import CryptoKit
import Foundation
import XCTest

final class BootstrapManifestTests: XCTestCase {
  func testSignedManifestValidatesAndTamperingFails() throws {
    let key = Curve25519.Signing.PrivateKey()
    let unsigned = manifest(signature: "")
    let signature = try key.signature(for: unsigned.signingPayload).base64EncodedString()
    let signed = manifest(signature: signature)

    XCTAssertEqual(
      try signed.validated(
        publicKeyHex: key.publicKey.rawRepresentation.hex,
        allowInsecureLocalhost: true
      ),
      signed
    )

    let tampered = BootstrapManifest(
      format: signed.format,
      version: "9.9.9",
      architecture: signed.architecture,
      artifactURL: signed.artifactURL,
      artifactSHA256: signed.artifactSHA256,
      artifactSize: signed.artifactSize,
      runtimeBundleName: signed.runtimeBundleName,
      signature: signed.signature
    )
    XCTAssertThrowsError(
      try tampered.validated(
        publicKeyHex: key.publicKey.rawRepresentation.hex,
        allowInsecureLocalhost: true
      )
    ) { error in
      XCTAssertEqual(error as? BootstrapError, .invalidSignature)
    }
  }

  func testProductionManifestRejectsPlainHTTP() throws {
    let key = Curve25519.Signing.PrivateKey()
    let unsigned = manifest(signature: "")
    let signature = try key.signature(for: unsigned.signingPayload).base64EncodedString()
    XCTAssertThrowsError(
      try manifest(signature: signature).validated(
        publicKeyHex: key.publicKey.rawRepresentation.hex)
    )
  }

  private func manifest(signature: String) -> BootstrapManifest {
    BootstrapManifest(
      format: 1,
      version: "1.2.3",
      architecture: "arm64",
      artifactURL: "http://127.0.0.1:8765/TerraneRuntime-arm64.zip",
      artifactSHA256: String(repeating: "a", count: 64),
      artifactSize: 42,
      runtimeBundleName: "Terrane.app",
      signature: signature
    )
  }
}

final class RuntimeStoreTests: XCTestCase {
  func testInstallActivateAndRollbackUseVersionedAtomicState() throws {
    let temporary = try TemporaryDirectory()
    let store = RuntimeStore(root: temporary.url.appendingPathComponent("store"))
    try store.prepare()

    let first = try makeRuntimeArchive(in: temporary.url, version: "1.0.0")
    let firstManifest = makeManifest(
      version: "1.0.0",
      archive: first,
      sha256: try RuntimeStore.sha256(url: first)
    )
    let firstApp = try store.install(archive: first, manifest: firstManifest)
    XCTAssertTrue(FileManager.default.fileExists(atPath: firstApp.path))
    _ = try store.activate(manifest: firstManifest)

    let second = try makeRuntimeArchive(in: temporary.url, version: "2.0.0")
    let secondManifest = makeManifest(
      version: "2.0.0",
      archive: second,
      sha256: try RuntimeStore.sha256(url: second)
    )
    _ = try store.install(archive: second, manifest: secondManifest)
    _ = try store.activate(manifest: secondManifest)
    XCTAssertEqual(store.readState()?.previousVersion, "1.0.0")

    let rolledBack = try store.rollBack(from: "2.0.0")
    XCTAssertEqual(rolledBack?.path, firstApp.path)
    XCTAssertEqual(store.readState()?.activeVersion, "1.0.0")
  }

  func testInstallRejectsHashMismatchBeforeExtraction() throws {
    let temporary = try TemporaryDirectory()
    let store = RuntimeStore(root: temporary.url.appendingPathComponent("store"))
    let archive = try makeRuntimeArchive(in: temporary.url, version: "1.0.0")
    let manifest = makeManifest(
      version: "1.0.0",
      archive: archive,
      sha256: String(repeating: "0", count: 64)
    )
    XCTAssertThrowsError(try store.install(archive: archive, manifest: manifest))
  }

  private func makeManifest(version: String, archive: URL, sha256: String) -> BootstrapManifest {
    let size = (try! FileManager.default.attributesOfItem(atPath: archive.path)[.size] as! NSNumber)
      .int64Value
    return BootstrapManifest(
      format: 1,
      version: version,
      architecture: "arm64",
      artifactURL: "https://example.invalid/TerraneRuntime-arm64.zip",
      artifactSHA256: sha256,
      artifactSize: size,
      runtimeBundleName: "Terrane.app",
      signature: ""
    )
  }

  private func makeRuntimeArchive(in directory: URL, version: String) throws -> URL {
    let source = directory.appendingPathComponent("source-\(version)", isDirectory: true)
    let app = source.appendingPathComponent("Terrane.app", isDirectory: true)
    let contents = app.appendingPathComponent("Contents", isDirectory: true)
    let macOS = contents.appendingPathComponent("MacOS", isDirectory: true)
    try FileManager.default.createDirectory(at: macOS, withIntermediateDirectories: true)
    let executable = macOS.appendingPathComponent("TerraneHost")
    try Data("#!/bin/sh\nexit 0\n".utf8).write(to: executable)
    try FileManager.default.setAttributes(
      [.posixPermissions: 0o755], ofItemAtPath: executable.path)
    let plist: [String: Any] = [
      "CFBundleExecutable": "TerraneHost",
      "CFBundleIdentifier": "com.terrane.runtime-test.\(version)",
      "CFBundleName": "Terrane",
      "CFBundlePackageType": "APPL",
      "CFBundleVersion": "1",
    ]
    let plistData = try PropertyListSerialization.data(
      fromPropertyList: plist, format: .xml, options: 0)
    try plistData.write(to: contents.appendingPathComponent("Info.plist"))
    try run("/usr/bin/codesign", ["--force", "--sign", "-", app.path])

    let archive = directory.appendingPathComponent("runtime-\(version).zip")
    try run("/usr/bin/ditto", ["-c", "-k", "--keepParent", app.path, archive.path])
    return archive
  }

  private func run(_ executable: String, _ arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    try process.run()
    process.waitUntilExit()
    XCTAssertEqual(process.terminationStatus, 0)
  }
}

private final class TemporaryDirectory {
  let url: URL

  init() throws {
    url = FileManager.default.temporaryDirectory
      .appendingPathComponent("terrane-bootstrap-tests-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
  }

  deinit {
    try? FileManager.default.removeItem(at: url)
  }
}

extension Data {
  fileprivate var hex: String {
    map { String(format: "%02x", $0) }.joined()
  }
}

extension BootstrapError: Equatable {
  static func == (lhs: BootstrapError, rhs: BootstrapError) -> Bool {
    switch (lhs, rhs) {
    case (.invalidSignature, .invalidSignature):
      return true
    case (.invalidConfiguration(let left), .invalidConfiguration(let right)),
      (.invalidManifest(let left), .invalidManifest(let right)),
      (.download(let left), .download(let right)),
      (.installation(let left), .installation(let right)),
      (.launch(let left), .launch(let right)):
      return left == right
    case (
      .artifactSize(let leftExpected, let leftActual),
      .artifactSize(let rightExpected, let rightActual)
    ):
      return leftExpected == rightExpected && leftActual == rightActual
    case (
      .artifactHash(let leftExpected, let leftActual),
      .artifactHash(let rightExpected, let rightActual)
    ):
      return leftExpected == rightExpected && leftActual == rightActual
    default:
      return false
    }
  }
}
