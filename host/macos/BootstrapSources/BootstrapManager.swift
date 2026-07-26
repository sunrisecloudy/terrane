import AppKit
import Foundation

struct BootstrapViewState: Equatable {
  enum Phase: Equatable {
    case checking
    case downloading
    case verifying
    case installing
    case launching
    case complete
    case failed
  }

  let phase: Phase
  let title: String
  let detail: String
  let progress: Double?
  let byteDetail: String?
  let retryAvailable: Bool
}

protocol BootstrapManagerDelegate: AnyObject {
  func bootstrapManager(_ manager: BootstrapManager, didUpdate state: BootstrapViewState)
  func bootstrapManagerDidComplete(_ manager: BootstrapManager)
}

final class BootstrapManager: NSObject, SegmentedDownloaderDelegate {
  weak var delegate: BootstrapManagerDelegate?

  private let configuration: BootstrapConfiguration
  private let store: RuntimeStore
  private let manifestSession: URLSession
  private var manifest: BootstrapManifest?
  private var downloader: SegmentedDownloader?
  private var runningApplication: NSRunningApplication?
  private var phaseTimer: Timer?
  private var phaseStartedAt: Date?

  init(configuration: BootstrapConfiguration, store: RuntimeStore? = nil) {
    self.configuration = configuration
    self.store = store ?? RuntimeStore(root: configuration.storeRoot)
    let sessionConfiguration = URLSessionConfiguration.ephemeral
    sessionConfiguration.waitsForConnectivity = true
    sessionConfiguration.timeoutIntervalForRequest = 60
    sessionConfiguration.timeoutIntervalForResource = 60 * 60
    sessionConfiguration.httpMaximumConnectionsPerHost = configuration.maximumDownloadConnections
    manifestSession = URLSession(configuration: sessionConfiguration)
    super.init()
  }

  func start() {
    update(
      phase: .checking,
      title: "Checking Terrane",
      detail: "Looking for the latest verified runtime…",
      progress: nil
    )
    do {
      try store.prepare()
    } catch {
      fail(error)
      return
    }
    fetchManifest()
  }

  func retry() {
    downloader?.cancel()
    downloader = nil
    start()
  }

  private func fetchManifest() {
    let request = URLRequest(
      url: configuration.manifestURL,
      cachePolicy: .reloadIgnoringLocalAndRemoteCacheData,
      timeoutInterval: 60
    )
    manifestSession.dataTask(with: request) { [weak self] data, response, error in
      guard let self else { return }
      if let error {
        self.useInstalledRuntimeOrFail(BootstrapError.download(error.localizedDescription))
        return
      }
      if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
        self.useInstalledRuntimeOrFail(
          BootstrapError.download("release server returned HTTP \(http.statusCode)"))
        return
      }
      do {
        guard let data else { throw BootstrapError.download("release manifest was empty") }
        let decoded = try JSONDecoder().decode(BootstrapManifest.self, from: data)
        self.manifest = try decoded.validated(
          publicKeyHex: self.configuration.publicKeyHex,
          allowInsecureLocalhost: self.configuration.allowInsecureLocalhost
        )
        self.handleManifest()
      } catch {
        self.useInstalledRuntimeOrFail(error)
      }
    }.resume()
  }

  private func handleManifest() {
    guard let manifest else { return }
    if let active = store.activeRuntime(), active.0.failedVersion == manifest.version {
      launch(app: active.1, version: active.0.activeVersion, newlyInstalled: false)
      return
    }
    if let active = store.activeRuntime(), active.0.activeVersion == manifest.version,
      active.0.activeSHA256.caseInsensitiveCompare(manifest.artifactSHA256) == .orderedSame
    {
      launch(app: active.1, version: active.0.activeVersion, newlyInstalled: false)
      return
    }
    let cached = store.downloadURL(for: manifest)
    if FileManager.default.fileExists(atPath: cached.path),
      (try? RuntimeStore.sha256(url: cached))?.caseInsensitiveCompare(manifest.artifactSHA256)
        == .orderedSame
    {
      verifyAndInstall(archive: cached)
      return
    }
    beginDownload()
  }

  private func beginDownload() {
    guard let manifest, let url = URL(string: manifest.artifactURL) else {
      fail(BootstrapError.invalidManifest("artifact URL is missing"))
      return
    }
    stopPhaseTimer()
    update(
      phase: .downloading,
      title: "Downloading Terrane",
      detail: "Starting up to \(configuration.maximumDownloadConnections) secure connections…",
      progress: 0,
      byteDetail: "0 of \(Self.formatBytes(manifest.artifactSize)) • measuring speed…"
    )
    let downloader = SegmentedDownloader(
      url: url,
      expectedSize: manifest.artifactSize,
      destination: store.downloadURL(for: manifest),
      partsDirectory: store.downloadPartsDirectory(for: manifest),
      connectionLimit: configuration.maximumDownloadConnections,
      stallTimeout: configuration.downloadStallTimeout,
      maximumRetries: configuration.maximumDownloadRetries
    )
    self.downloader = downloader
    downloader.delegate = self
    downloader.start()
  }

  func segmentedDownloader(
    _ downloader: SegmentedDownloader, didUpdate progress: TransferProgress
  ) {
    guard downloader === self.downloader else { return }
    let fraction = min(
      1, max(0, Double(progress.receivedBytes) / Double(progress.totalBytes)))
    let speed =
      progress.bytesPerSecond.map { "\(Self.formatBytes(Int64($0)))/s" }
      ?? "measuring speed…"
    let eta =
      progress.estimatedSecondsRemaining.map { "\(Self.formatDuration($0)) remaining" }
      ?? "estimating time…"
    let connections =
      progress.activeConnections == 1
      ? "1 connection" : "\(progress.activeConnections) connections"
    update(
      phase: .downloading,
      title: "Downloading Terrane",
      detail: "\(connections) • \(Self.formatDuration(progress.elapsed)) elapsed",
      progress: fraction,
      byteDetail:
        "\(Self.formatBytes(progress.receivedBytes)) of \(Self.formatBytes(progress.totalBytes))"
        + " • \(speed) • \(eta)"
    )
  }

  func segmentedDownloader(
    _ downloader: SegmentedDownloader,
    didRetry reason: String,
    attempt: Int,
    maximum: Int
  ) {
    guard downloader === self.downloader, let manifest else { return }
    update(
      phase: .downloading,
      title: "Downloading Terrane",
      detail: "\(reason) (\(attempt)/\(maximum))",
      progress: nil,
      byteDetail: "Keeping verified partial data • \(Self.formatBytes(manifest.artifactSize)) total"
    )
  }

  func segmentedDownloader(
    _ downloader: SegmentedDownloader,
    didComplete result: Result<URL, Error>
  ) {
    guard downloader === self.downloader else { return }
    self.downloader = nil
    switch result {
    case .success(let archive):
      verifyAndInstall(archive: archive)
    case .failure(let error):
      fail(error)
    }
  }

  private func verifyAndInstall(archive: URL) {
    guard let manifest else { return }
    beginTimedPhase(
      phase: .verifying,
      title: "Verifying Terrane",
      detail: "Checking signature, size, and archive integrity…"
    )
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      guard let self else { return }
      do {
        let app = try self.store.install(archive: archive, manifest: manifest)
        self.beginTimedPhase(
          phase: .installing,
          title: "Finishing installation",
          detail: "Activating Terrane \(manifest.version)…"
        )
        _ = try self.store.activate(manifest: manifest)
        if self.configuration.skipRuntimeLaunch {
          self.stopPhaseTimer()
          self.update(
            phase: .complete,
            title: "Terrane is ready",
            detail: "The verified runtime was installed successfully.",
            progress: 1
          )
          DispatchQueue.main.async { self.delegate?.bootstrapManagerDidComplete(self) }
        } else {
          self.launch(app: app, version: manifest.version, newlyInstalled: true)
        }
      } catch {
        self.fail(error)
      }
    }
  }

  private func launch(app: URL, version: String, newlyInstalled: Bool) {
    stopPhaseTimer()
    update(
      phase: .launching,
      title: "Opening Terrane",
      detail: newlyInstalled ? "Confirming the new runtime starts correctly…" : "Starting Terrane…",
      progress: nil
    )
    if let existing = NSRunningApplication.runningApplications(
      withBundleIdentifier: "com.terrane.host"
    ).first {
      existing.activate(options: [.activateAllWindows])
      update(
        phase: .complete,
        title: "Terrane is ready",
        detail: "Terrane is already running.",
        progress: 1
      )
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
        guard let self else { return }
        self.delegate?.bootstrapManagerDidComplete(self)
      }
      return
    }
    let healthFile = store.root.appendingPathComponent(
      ".health-\(version)-\(UUID().uuidString)")
    try? FileManager.default.removeItem(at: healthFile)
    let openConfiguration = NSWorkspace.OpenConfiguration()
    openConfiguration.activates = true
    openConfiguration.createsNewApplicationInstance = false
    var environment = [
      "TERRANE_BOOTSTRAP_HEALTH_FILE": healthFile.path
    ]
    if let runtimeHome = configuration.runtimeHome {
      environment["TERRANE_HOME"] = runtimeHome
    }
    openConfiguration.environment = environment
    NSWorkspace.shared.openApplication(at: app, configuration: openConfiguration) {
      [weak self] application, error in
      guard let self else { return }
      if let error {
        self.handleLaunchFailure(
          version: version, error: BootstrapError.launch(error.localizedDescription))
        return
      }
      self.runningApplication = application
      self.waitForHealth(
        file: healthFile, version: version,
        deadline: Date().addingTimeInterval(
          self.configuration.healthTimeout))
    }
  }

  private func waitForHealth(file: URL, version: String, deadline: Date) {
    if FileManager.default.fileExists(atPath: file.path) {
      try? FileManager.default.removeItem(at: file)
      update(
        phase: .complete,
        title: "Terrane is ready",
        detail: "The verified runtime opened successfully.",
        progress: 1
      )
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
        guard let self else { return }
        self.delegate?.bootstrapManagerDidComplete(self)
      }
      return
    }
    guard Date() < deadline else {
      runningApplication?.terminate()
      handleLaunchFailure(
        version: version,
        error: BootstrapError.launch("the runtime did not report healthy startup in time")
      )
      return
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
      self?.waitForHealth(file: file, version: version, deadline: deadline)
    }
  }

  private func handleLaunchFailure(version: String, error: Error) {
    do {
      if let previous = try store.rollBack(from: version) {
        launch(
          app: previous, version: store.readState()?.activeVersion ?? "previous",
          newlyInstalled: false)
      } else {
        fail(error)
      }
    } catch {
      fail(error)
    }
  }

  private func useInstalledRuntimeOrFail(_ error: Error) {
    if let active = store.activeRuntime(), !configuration.skipRuntimeLaunch {
      launch(app: active.1, version: active.0.activeVersion, newlyInstalled: false)
    } else {
      fail(error)
    }
  }

  private func fail(_ error: Error) {
    stopPhaseTimer()
    let message = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    update(
      phase: .failed,
      title: "Terrane needs another try",
      detail: message,
      progress: nil,
      retryAvailable: true
    )
  }

  private func update(
    phase: BootstrapViewState.Phase,
    title: String,
    detail: String,
    progress: Double?,
    byteDetail: String? = nil,
    retryAvailable: Bool = false
  ) {
    let state = BootstrapViewState(
      phase: phase,
      title: title,
      detail: detail,
      progress: progress,
      byteDetail: byteDetail,
      retryAvailable: retryAvailable
    )
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.delegate?.bootstrapManager(self, didUpdate: state)
    }
  }

  private static func formatBytes(_ bytes: Int64) -> String {
    ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
  }

  private static func formatDuration(_ seconds: TimeInterval) -> String {
    let value = max(0, seconds)
    if value < 10 {
      return String(format: "%.1fs", value)
    }
    if value < 60 {
      return "\(Int(value.rounded()))s"
    }
    let minutes = Int(value) / 60
    let remaining = Int(value) % 60
    return "\(minutes)m \(remaining)s"
  }

  private func beginTimedPhase(
    phase: BootstrapViewState.Phase,
    title: String,
    detail: String
  ) {
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      self.stopPhaseTimerOnMain()
      let started = Date()
      self.phaseStartedAt = started
      self.update(
        phase: phase,
        title: title,
        detail: detail,
        progress: nil,
        byteDetail: "0.0s elapsed"
      )
      self.phaseTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) {
        [weak self] _ in
        guard let self, self.phaseStartedAt == started else { return }
        self.update(
          phase: phase,
          title: title,
          detail: detail,
          progress: nil,
          byteDetail: "\(Self.formatDuration(Date().timeIntervalSince(started))) elapsed"
        )
      }
    }
  }

  private func stopPhaseTimer() {
    DispatchQueue.main.async { [weak self] in
      self?.stopPhaseTimerOnMain()
    }
  }

  private func stopPhaseTimerOnMain() {
    phaseTimer?.invalidate()
    phaseTimer = nil
    phaseStartedAt = nil
  }
}
