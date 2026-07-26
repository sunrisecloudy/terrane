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

final class BootstrapManager: NSObject, URLSessionDownloadDelegate {
  weak var delegate: BootstrapManagerDelegate?

  private let configuration: BootstrapConfiguration
  private let store: RuntimeStore
  private var session: URLSession!
  private var manifest: BootstrapManifest?
  private var downloadedTemporaryURL: URL?
  private var completionError: Error?
  private var didFinishDownload = false
  private var runningApplication: NSRunningApplication?

  init(configuration: BootstrapConfiguration, store: RuntimeStore? = nil) {
    self.configuration = configuration
    self.store = store ?? RuntimeStore(root: configuration.storeRoot)
    super.init()
    let sessionConfiguration = URLSessionConfiguration.ephemeral
    sessionConfiguration.waitsForConnectivity = true
    sessionConfiguration.timeoutIntervalForRequest = 60
    sessionConfiguration.timeoutIntervalForResource = 60 * 60
    sessionConfiguration.httpMaximumConnectionsPerHost = 2
    session = URLSession(
      configuration: sessionConfiguration,
      delegate: self,
      delegateQueue: OperationQueue()
    )
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
    completionError = nil
    downloadedTemporaryURL = nil
    didFinishDownload = false
    start()
  }

  private func fetchManifest() {
    let request = URLRequest(
      url: configuration.manifestURL,
      cachePolicy: .reloadIgnoringLocalAndRemoteCacheData,
      timeoutInterval: 60
    )
    session.dataTask(with: request) { [weak self] data, response, error in
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
    update(
      phase: .downloading,
      title: "Downloading Terrane",
      detail: "You can keep using your Mac while Terrane gets ready.",
      progress: 0,
      byteDetail: "0 of \(Self.formatBytes(manifest.artifactSize))"
    )
    if let resumeData = try? Data(contentsOf: store.resumeDataURL(for: manifest)),
      !resumeData.isEmpty
    {
      session.downloadTask(withResumeData: resumeData).resume()
    } else {
      session.downloadTask(with: URLRequest(url: url)).resume()
    }
  }

  func urlSession(
    _ session: URLSession,
    downloadTask: URLSessionDownloadTask,
    didWriteData bytesWritten: Int64,
    totalBytesWritten: Int64,
    totalBytesExpectedToWrite: Int64
  ) {
    guard let manifest else { return }
    let expected = manifest.artifactSize
    guard totalBytesWritten <= expected + 1024 * 1024 else {
      downloadTask.cancel()
      completionError = BootstrapError.artifactSize(
        expected: expected, actual: totalBytesWritten)
      return
    }
    let progress = min(1, max(0, Double(totalBytesWritten) / Double(expected)))
    update(
      phase: .downloading,
      title: "Downloading Terrane",
      detail: "You can keep using your Mac while Terrane gets ready.",
      progress: progress,
      byteDetail:
        "\(Self.formatBytes(totalBytesWritten)) of \(Self.formatBytes(expected))"
    )
  }

  func urlSession(
    _ session: URLSession,
    downloadTask: URLSessionDownloadTask,
    didFinishDownloadingTo location: URL
  ) {
    guard let manifest else { return }
    if let response = downloadTask.response as? HTTPURLResponse,
      !(200...299).contains(response.statusCode)
    {
      completionError = BootstrapError.download(
        "release server returned HTTP \(response.statusCode)")
      return
    }
    do {
      let destination = store.downloadURL(for: manifest)
      if FileManager.default.fileExists(atPath: destination.path) {
        try FileManager.default.removeItem(at: destination)
      }
      try FileManager.default.moveItem(at: location, to: destination)
      downloadedTemporaryURL = destination
      didFinishDownload = true
      store.removeResumeData(for: manifest)
    } catch {
      completionError = error
    }
  }

  func urlSession(
    _ session: URLSession,
    task: URLSessionTask,
    didCompleteWithError error: Error?
  ) {
    guard task is URLSessionDownloadTask else { return }
    if let resumeData = (error as NSError?)?.userInfo[
      NSURLSessionDownloadTaskResumeData] as? Data
    {
      if let manifest {
        try? resumeData.write(to: store.resumeDataURL(for: manifest), options: .atomic)
      }
    }
    if let completionError {
      fail(completionError)
    } else if let error {
      fail(BootstrapError.download(error.localizedDescription))
    } else if didFinishDownload, let archive = downloadedTemporaryURL {
      verifyAndInstall(archive: archive)
    }
  }

  private func verifyAndInstall(archive: URL) {
    guard let manifest else { return }
    update(
      phase: .verifying,
      title: "Verifying Terrane",
      detail: "Checking the signed release before it is installed…",
      progress: nil
    )
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      guard let self else { return }
      do {
        let app = try self.store.install(archive: archive, manifest: manifest)
        self.update(
          phase: .installing,
          title: "Finishing installation",
          detail: "Activating Terrane \(manifest.version)…",
          progress: nil
        )
        _ = try self.store.activate(manifest: manifest)
        if self.configuration.skipRuntimeLaunch {
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
}
