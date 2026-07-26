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

final class SegmentedDownloaderTests: XCTestCase {
  func testDownloaderAssemblesEightValidatedRanges() throws {
    let temporary = try TemporaryDirectory()
    let payload = Data((0..<65_537).map { UInt8($0 % 251) })
    RangeURLProtocol.configure(payload: payload, stallFirstWave: false)
    let delegate = DownloaderTestDelegate()
    let completed = expectation(description: "download completed")
    delegate.completionExpectation = completed
    let destination = temporary.url.appendingPathComponent("runtime.zip")
    let downloader = SegmentedDownloader(
      url: URL(string: "https://fixture.invalid/runtime.zip")!,
      expectedSize: Int64(payload.count),
      destination: destination,
      partsDirectory: temporary.url.appendingPathComponent("parts"),
      connectionLimit: 8,
      protocolClasses: [RangeURLProtocol.self]
    )
    downloader.delegate = delegate

    downloader.start()
    wait(for: [completed], timeout: 5)

    XCTAssertEqual(try Data(contentsOf: destination), payload)
    XCTAssertEqual(RangeURLProtocol.initialRangeRequestCount, 8)
    XCTAssertNil(delegate.error)
  }

  func testDownloaderDetectsGlobalStallAndResumesParts() throws {
    let temporary = try TemporaryDirectory()
    let payload = Data((0..<65_537).map { UInt8($0 % 239) })
    RangeURLProtocol.configure(payload: payload, stallFirstWave: true)
    let delegate = DownloaderTestDelegate()
    let retried = expectation(description: "stall retried")
    let completed = expectation(description: "download completed")
    delegate.retryExpectation = retried
    delegate.completionExpectation = completed
    let destination = temporary.url.appendingPathComponent("runtime.zip")
    let downloader = SegmentedDownloader(
      url: URL(string: "https://fixture.invalid/runtime.zip")!,
      expectedSize: Int64(payload.count),
      destination: destination,
      partsDirectory: temporary.url.appendingPathComponent("parts"),
      connectionLimit: 8,
      stallTimeout: 2,
      maximumRetries: 2,
      protocolClasses: [RangeURLProtocol.self]
    )
    downloader.delegate = delegate

    downloader.start()
    wait(for: [retried, completed], timeout: 8)

    XCTAssertEqual(try Data(contentsOf: destination), payload)
    XCTAssertNil(delegate.error)
  }

  func testSegmentPlanUsesAtMostEightContiguousRanges() {
    let segments = DownloadSegment.plan(size: 18_418_459, connectionLimit: 99)

    XCTAssertEqual(segments.count, 8)
    XCTAssertEqual(segments.first?.start, 0)
    XCTAssertEqual(segments.last?.end, 18_418_458)
    XCTAssertEqual(segments.reduce(0) { $0 + $1.length }, 18_418_459)
    for pair in zip(segments, segments.dropFirst()) {
      XCTAssertEqual(pair.0.end + 1, pair.1.start)
    }
  }

  func testSegmentPlanDoesNotCreateEmptyRangesForSmallFiles() {
    XCTAssertEqual(
      DownloadSegment.plan(size: 3, connectionLimit: 8),
      [
        DownloadSegment(index: 0, start: 0, end: 0),
        DownloadSegment(index: 1, start: 1, end: 1),
        DownloadSegment(index: 2, start: 2, end: 2),
      ])
  }

  func testRateEstimatorUsesRecentByteWindow() {
    let start = Date(timeIntervalSince1970: 1_000)
    var estimator = TransferRateEstimator(window: 5)
    estimator.record(totalBytes: 0, at: start)
    estimator.record(totalBytes: 2_000_000, at: start.addingTimeInterval(2))

    XCTAssertEqual(
      estimator.rate(at: start.addingTimeInterval(2)) ?? 0,
      1_000_000,
      accuracy: 0.001)
  }

  func testConfigurationBoundsConnectionAndRetrySettings() throws {
    let configuration = try BootstrapConfiguration.resolve(
      environment: [
        "TERRANE_BOOTSTRAP_CONNECTIONS": "99",
        "TERRANE_BOOTSTRAP_STALL_TIMEOUT": "0.1",
        "TERRANE_BOOTSTRAP_MAX_RETRIES": "99",
      ],
      applicationSupport: FileManager.default.temporaryDirectory
    )

    XCTAssertEqual(configuration.maximumDownloadConnections, 8)
    XCTAssertEqual(configuration.downloadStallTimeout, 2)
    XCTAssertEqual(configuration.maximumDownloadRetries, 10)
  }
}

private final class DownloaderTestDelegate: SegmentedDownloaderDelegate {
  var completionExpectation: XCTestExpectation?
  var retryExpectation: XCTestExpectation?
  var error: Error?

  func segmentedDownloader(
    _ downloader: SegmentedDownloader, didUpdate progress: TransferProgress
  ) {}

  func segmentedDownloader(
    _ downloader: SegmentedDownloader,
    didRetry reason: String,
    attempt: Int,
    maximum: Int
  ) {
    if reason.contains("stalled") {
      retryExpectation?.fulfill()
      retryExpectation = nil
    }
  }

  func segmentedDownloader(
    _ downloader: SegmentedDownloader,
    didComplete result: Result<URL, Error>
  ) {
    if case .failure(let error) = result {
      self.error = error
    }
    completionExpectation?.fulfill()
  }
}

private final class RangeURLProtocol: URLProtocol {
  private static let lock = NSLock()
  private static var payload = Data()
  private static var shouldStallFirstWave = false
  private static var rangeRequests = 0

  static var initialRangeRequestCount: Int {
    lock.lock()
    defer { lock.unlock() }
    return rangeRequests
  }

  static func configure(payload: Data, stallFirstWave: Bool) {
    lock.lock()
    self.payload = payload
    shouldStallFirstWave = stallFirstWave
    rangeRequests = 0
    lock.unlock()
  }

  override class func canInit(with request: URLRequest) -> Bool { true }

  override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

  override func startLoading() {
    Self.lock.lock()
    let payload = Self.payload
    let shouldStall = Self.shouldStallFirstWave && Self.rangeRequests < 8
    if request.value(forHTTPHeaderField: "Range") != nil {
      Self.rangeRequests += 1
    }
    Self.lock.unlock()

    if request.httpMethod == "HEAD" {
      respond(
        status: 200,
        headers: [
          "Accept-Ranges": "bytes",
          "Content-Length": String(payload.count),
        ],
        data: nil,
        finish: true
      )
      return
    }

    guard let range = request.value(forHTTPHeaderField: "Range"),
      let bounds = Self.parse(range: range),
      bounds.start >= 0,
      bounds.end < payload.count,
      bounds.start <= bounds.end
    else {
      respond(status: 400, headers: [:], data: nil, finish: true)
      return
    }
    let requested = payload.subdata(in: bounds.start..<(bounds.end + 1))
    let delivered =
      shouldStall ? Data(requested.prefix(min(256, requested.count))) : requested
    respond(
      status: 206,
      headers: [
        "Accept-Ranges": "bytes",
        "Content-Length": String(requested.count),
        "Content-Range": "bytes \(bounds.start)-\(bounds.end)/\(payload.count)",
      ],
      data: delivered,
      finish: !shouldStall
    )
  }

  override func stopLoading() {}

  private func respond(
    status: Int,
    headers: [String: String],
    data: Data?,
    finish: Bool
  ) {
    let response = HTTPURLResponse(
      url: request.url!,
      statusCode: status,
      httpVersion: "HTTP/1.1",
      headerFields: headers
    )!
    client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
    if let data {
      client?.urlProtocol(self, didLoad: data)
    }
    if finish {
      client?.urlProtocolDidFinishLoading(self)
    }
  }

  private static func parse(range: String) -> (start: Int, end: Int)? {
    let value = range.replacingOccurrences(of: "bytes=", with: "")
    let pieces = value.split(separator: "-", omittingEmptySubsequences: false)
    guard pieces.count == 2, let start = Int(pieces[0]), let end = Int(pieces[1]) else {
      return nil
    }
    return (start, end)
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
