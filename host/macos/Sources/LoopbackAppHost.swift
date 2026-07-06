import Foundation

final class LoopbackAppHost {
  private let loopbackHome: URL
  private let appsDirectory: URL
  private let binaryURL: URL
  private var process: Process?
  private var baseURL: URL?

  init?(home: URL, appsDirectory: URL) {
    guard let repo = Self.resolveRepoRoot(),
      let binary = Self.findTerraneWebBinary(repo: repo)
    else {
      return nil
    }
    self.loopbackHome = FileManager.default.temporaryDirectory
      .appendingPathComponent("terrane-loopback-\(ProcessInfo.processInfo.processIdentifier)")
    self.appsDirectory = appsDirectory
    self.binaryURL = binary
  }

  func frameURL(for app: TerraneApp) -> URL? {
    guard let baseURL = startIfNeeded() else { return nil }
    var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
    components?.path = "/apps/\(app.id)/__terrane/frame/"
    return components?.url
  }

  func stop() {
    guard let process else { return }
    if process.isRunning {
      process.terminate()
    }
    self.process = nil
    self.baseURL = nil
  }

  private func startIfNeeded() -> URL? {
    if let baseURL, process?.isRunning == true {
      return baseURL
    }

    let ready = DispatchSemaphore(value: 0)
    let stderr = Pipe()
    let process = Process()
    process.executableURL = binaryURL
    process.arguments = [
      "--addr", "127.0.0.1:0",
      "--apps", appsDirectory.path,
      "--no-live-reload",
    ]
    var environment = ProcessInfo.processInfo.environment
    try? FileManager.default.createDirectory(
      at: loopbackHome,
      withIntermediateDirectories: true
    )
    environment["TERRANE_HOME"] = loopbackHome.path
    environment["TERRANE_I18N_DIR"] = appsDirectory.deletingLastPathComponent().path
    process.environment = environment
    process.standardError = stderr

    var buffered = ""
    var discoveredURL: URL?
    stderr.fileHandleForReading.readabilityHandler = { handle in
      let data = handle.availableData
      guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
      buffered += text
      if discoveredURL == nil, let url = Self.parseListenURL(from: buffered) {
        discoveredURL = url
        ready.signal()
      }
    }

    do {
      try process.run()
    } catch {
      stderr.fileHandleForReading.readabilityHandler = nil
      return nil
    }

    _ = ready.wait(timeout: .now() + 5)
    stderr.fileHandleForReading.readabilityHandler = nil

    guard let discoveredURL, process.isRunning else {
      if process.isRunning {
        process.terminate()
      }
      return nil
    }

    self.process = process
    self.baseURL = discoveredURL
    return discoveredURL
  }

  private static func parseListenURL(from text: String) -> URL? {
    guard let range = text.range(of: #"http://127\.0\.0\.1:\d+"#, options: .regularExpression)
    else {
      return nil
    }
    let listenURL = String(text[range]).replacingOccurrences(
      of: "http://127.0.0.1:",
      with: "http://localhost:"
    )
    return URL(string: listenURL)
  }

  private static func resolveRepoRoot() -> URL? {
    if let repo = ProcessInfo.processInfo.environment["TERRANE_REPO"]?
      .trimmingCharacters(in: .whitespacesAndNewlines),
      !repo.isEmpty
    {
      return URL(fileURLWithPath: repo)
    }
    let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    if FileManager.default.fileExists(atPath: cwd.appendingPathComponent("apps").path) {
      return cwd
    }
    return nil
  }

  private static func findTerraneWebBinary(repo: URL) -> URL? {
    let candidates = [
      repo.appendingPathComponent("target/release/terrane-web"),
      repo.appendingPathComponent("target/debug/terrane-web"),
    ]
    return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }
  }
}
