import CryptoKit
import Foundation

struct RuntimeInstallState: Codable, Equatable {
  let activeVersion: String
  let activeSHA256: String
  let previousVersion: String?
  let previousSHA256: String?
  let failedVersion: String?
  let failedAt: Date?

  init(
    activeVersion: String,
    activeSHA256: String,
    previousVersion: String?,
    previousSHA256: String? = nil,
    failedVersion: String? = nil,
    failedAt: Date? = nil
  ) {
    self.activeVersion = activeVersion
    self.activeSHA256 = activeSHA256
    self.previousVersion = previousVersion
    self.previousSHA256 = previousSHA256
    self.failedVersion = failedVersion
    self.failedAt = failedAt
  }
}

final class RuntimeStore {
  let root: URL
  private let fileManager: FileManager

  init(root: URL, fileManager: FileManager = .default) {
    self.root = root
    self.fileManager = fileManager
  }

  var stateURL: URL { root.appendingPathComponent("runtime-state.json") }
  var downloadsDirectory: URL { root.appendingPathComponent("downloads", isDirectory: true) }
  var versionsDirectory: URL { root.appendingPathComponent("versions", isDirectory: true) }
  var stagingDirectory: URL { root.appendingPathComponent("staging", isDirectory: true) }
  func resumeDataURL(for manifest: BootstrapManifest) -> URL {
    downloadsDirectory.appendingPathComponent(
      "\(manifest.artifactSHA256.lowercased()).resume")
  }

  func prepare() throws {
    for directory in [root, downloadsDirectory, versionsDirectory, stagingDirectory] {
      try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
    }
  }

  func readState() -> RuntimeInstallState? {
    guard let data = try? Data(contentsOf: stateURL) else { return nil }
    return try? JSONDecoder().decode(RuntimeInstallState.self, from: data)
  }

  func activeRuntime() -> (RuntimeInstallState, URL)? {
    guard let state = readState() else { return nil }
    let app = installedApp(version: state.activeVersion)
    return fileManager.fileExists(atPath: app.path) ? (state, app) : nil
  }

  func installedApp(version: String) -> URL {
    versionsDirectory
      .appendingPathComponent(safeVersionDirectory(version), isDirectory: true)
      .appendingPathComponent("Terrane.app", isDirectory: true)
  }

  func downloadURL(for manifest: BootstrapManifest) -> URL {
    downloadsDirectory.appendingPathComponent("\(manifest.artifactSHA256.lowercased()).zip")
  }

  func install(archive: URL, manifest: BootstrapManifest) throws -> URL {
    try prepare()
    let attributes = try fileManager.attributesOfItem(atPath: archive.path)
    let actualSize = (attributes[.size] as? NSNumber)?.int64Value ?? -1
    guard actualSize == manifest.artifactSize else {
      throw BootstrapError.artifactSize(expected: manifest.artifactSize, actual: actualSize)
    }
    let actualHash = try Self.sha256(url: archive)
    guard actualHash.caseInsensitiveCompare(manifest.artifactSHA256) == .orderedSame else {
      throw BootstrapError.artifactHash(expected: manifest.artifactSHA256, actual: actualHash)
    }

    let nonce = UUID().uuidString
    let extraction = stagingDirectory.appendingPathComponent(nonce, isDirectory: true)
    try fileManager.createDirectory(at: extraction, withIntermediateDirectories: true)
    defer { try? fileManager.removeItem(at: extraction) }

    try run("/usr/bin/ditto", arguments: ["-x", "-k", archive.path, extraction.path])
    let expected = extraction.appendingPathComponent(manifest.runtimeBundleName, isDirectory: true)
    guard fileManager.fileExists(atPath: expected.path) else {
      throw BootstrapError.installation(
        "archive does not contain \(manifest.runtimeBundleName) at its root")
    }
    let entries = try fileManager.contentsOfDirectory(
      at: extraction, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
    guard entries.count == 1, entries[0].lastPathComponent == manifest.runtimeBundleName else {
      throw BootstrapError.installation("archive contains unexpected top-level files")
    }
    try run(
      "/usr/bin/codesign",
      arguments: ["--verify", "--deep", "--strict", "--verbose=2", expected.path])

    let versionDirectory =
      versionsDirectory
      .appendingPathComponent(safeVersionDirectory(manifest.version), isDirectory: true)
    let temporaryVersion =
      versionsDirectory
      .appendingPathComponent(
        ".\(safeVersionDirectory(manifest.version)).\(nonce)", isDirectory: true)
    try fileManager.createDirectory(at: temporaryVersion, withIntermediateDirectories: true)
    let temporaryApp = temporaryVersion.appendingPathComponent("Terrane.app", isDirectory: true)
    do {
      try fileManager.moveItem(at: expected, to: temporaryApp)
      if fileManager.fileExists(atPath: versionDirectory.path) {
        try fileManager.removeItem(at: versionDirectory)
      }
      try fileManager.moveItem(at: temporaryVersion, to: versionDirectory)
    } catch {
      try? fileManager.removeItem(at: temporaryVersion)
      throw BootstrapError.installation(error.localizedDescription)
    }
    return versionDirectory.appendingPathComponent("Terrane.app", isDirectory: true)
  }

  @discardableResult
  func activate(manifest: BootstrapManifest) throws -> RuntimeInstallState {
    let existing = readState()
    let previous = existing?.activeVersion
    let state = RuntimeInstallState(
      activeVersion: manifest.version,
      activeSHA256: manifest.artifactSHA256.lowercased(),
      previousVersion: previous == manifest.version ? existing?.previousVersion : previous,
      previousSHA256: previous == manifest.version
        ? existing?.previousSHA256
        : existing?.activeSHA256
    )
    try writeState(state)
    return state
  }

  func rollBack(from failedVersion: String) throws -> URL? {
    guard let state = readState(), state.activeVersion == failedVersion,
      let previous = state.previousVersion
    else {
      return nil
    }
    let previousApp = installedApp(version: previous)
    guard fileManager.fileExists(atPath: previousApp.path) else { return nil }
    let restored = RuntimeInstallState(
      activeVersion: previous,
      activeSHA256: state.previousSHA256 ?? "",
      previousVersion: nil,
      failedVersion: failedVersion,
      failedAt: Date()
    )
    try writeState(restored)
    return previousApp
  }

  func removeResumeData(for manifest: BootstrapManifest) {
    try? fileManager.removeItem(at: resumeDataURL(for: manifest))
  }

  static func sha256(url: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var hasher = SHA256()
    while true {
      let data = try handle.read(upToCount: 1024 * 1024) ?? Data()
      if data.isEmpty { break }
      hasher.update(data: data)
    }
    return hasher.finalize().map { String(format: "%02x", $0) }.joined()
  }

  private func writeState(_ state: RuntimeInstallState) throws {
    try prepare()
    let data = try JSONEncoder.pretty.encode(state)
    let temporary = root.appendingPathComponent(".runtime-state.\(UUID().uuidString).tmp")
    try data.write(to: temporary, options: .atomic)
    if fileManager.fileExists(atPath: stateURL.path) {
      _ = try fileManager.replaceItemAt(stateURL, withItemAt: temporary)
    } else {
      try fileManager.moveItem(at: temporary, to: stateURL)
    }
  }

  private func safeVersionDirectory(_ value: String) -> String {
    let safe = value.map { character -> Character in
      character.isLetter || character.isNumber || ".-_".contains(character) ? character : "_"
    }
    return String(safe.prefix(128))
  }

  private func run(_ executable: String, arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    let errorPipe = Pipe()
    process.standardError = errorPipe
    do {
      try process.run()
      process.waitUntilExit()
    } catch {
      throw BootstrapError.installation(error.localizedDescription)
    }
    guard process.terminationStatus == 0 else {
      let data = errorPipe.fileHandleForReading.readDataToEndOfFile()
      let message = String(data: data, encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines)
      throw BootstrapError.installation(
        message?.isEmpty == false
          ? message! : "\(executable) exited with \(process.terminationStatus)"
      )
    }
  }
}

extension JSONEncoder {
  fileprivate static var pretty: JSONEncoder {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    return encoder
  }
}
