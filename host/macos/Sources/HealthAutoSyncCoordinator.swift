import Foundation
import TerranePremiumSession

@MainActor
final class MacHealthAutoSyncCoordinator {
  private let client: PremiumHealthImageSyncClient
  private let blobImporter: TerraneBlobImporter
  private weak var bridge: TerraneBridge?
  private var task: Task<Void, Never>?
  private var lastErrorDescription: String?

  private(set) var connections: PremiumHealthConnections?
  private(set) var currentDeviceID: String?
  var onNutritionReady: ((String, String) -> Void)?
  var onConnectionsChanged: (() -> Void)?

  init(session: PremiumSessionClient, bridge: TerraneBridge) {
    let imageKeyStore: any PremiumHealthImageKeyStore
    let deviceKeyStore: any PremiumHealthDeviceKeyStore
    #if DEBUG
      if ProcessInfo.processInfo.environment[
        "TERRANE_E2E_HEALTH_VOLATILE_KEYS"
      ] == "1", let store = try? PremiumVolatileHealthImageKeyStore() {
        imageKeyStore = store
      } else {
        imageKeyStore = PremiumKeychainHealthImageKeyStore(
          service: "com.terrane.health.sync.macos"
        )
      }
      if let encoded = ProcessInfo.processInfo.environment[
        "TERRANE_E2E_HEALTH_DEVICE_KEY_BASE64"
      ],
        let key = Data(base64Encoded: encoded),
        let store = try? PremiumStaticHealthDeviceKeyStore(privateKey: key)
      {
        deviceKeyStore = store
      } else {
        deviceKeyStore = PremiumKeychainHealthDeviceKeyStore(
          service: "com.terrane.health.sync.macos"
        )
      }
    #else
      imageKeyStore = PremiumKeychainHealthImageKeyStore(
        service: "com.terrane.health.sync.macos"
      )
      deviceKeyStore = PremiumKeychainHealthDeviceKeyStore(
        service: "com.terrane.health.sync.macos"
      )
    #endif
    client = PremiumHealthImageSyncClient(
      session: session,
      keyStore: imageKeyStore,
      deviceKeyStore: deviceKeyStore,
      platform: .macOS
    )
    blobImporter = TerraneBlobImporter(handle: bridge.terraneHandle)
    self.bridge = bridge
  }

  func start() {
    guard task == nil else { return }
    task = Task { [weak self] in
      guard let self else { return }
      while !Task.isCancelled {
        await self.pollOnce()
        try? await Task.sleep(for: .seconds(1))
      }
    }
  }

  func stop() {
    task?.cancel()
    task = nil
    Task { [client] in
      try? await client.heartbeatAnalyzer(ready: false)
    }
  }

  func refreshConnections() async {
    do {
      async let deviceID = client.currentDeviceID()
      async let latest = client.connections()
      currentDeviceID = try await deviceID
      connections = try await latest
      onConnectionsChanged?()
    } catch {
      record(error)
    }
  }

  func approve(senderDeviceID: String) async throws {
    try await client.approveSender(deviceID: senderDeviceID)
    await refreshConnections()
  }

  func revoke(senderDeviceID: String) async throws {
    _ = try await client.revokeSender(deviceID: senderDeviceID)
    await refreshConnections()
  }

  func isApproved(senderDeviceID: String) -> Bool {
    guard let currentDeviceID else { return false }
    return connections?.pairings.contains {
      $0.senderDeviceId == senderDeviceID
        && $0.recipientDeviceId == currentDeviceID
        && $0.status == "approved"
    } == true
  }

  private func pollOnce() async {
    do {
      _ = try await client.heartbeatAnalyzer(ready: true)
      await refreshConnections()
      let claim = try await client.claimAnalysisJob(waitSeconds: 0)
      guard let job = claim.job, let lease = claim.lease else {
        lastErrorDescription = nil
        return
      }
      try await process(job: job, lease: lease)
      lastErrorDescription = nil
    } catch {
      record(error)
    }
  }

  private func process(
    job: PremiumHealthAnalysisJob,
    lease: PremiumHealthAnalysisLease
  ) async throws {
    let keepAlive = Task { [client] in
      while !Task.isCancelled {
        try? await Task.sleep(for: .seconds(10))
        guard !Task.isCancelled else { return }
        _ = try? await client.heartbeatAnalysisJob(job, lease: lease)
      }
    }
    do {
      guard
        let attachment = try await client.list().first(where: {
          $0.id == job.sourceImageId
        })
      else {
        throw PremiumHealthImageSyncError.invalidResponse
      }
      let image = try await client.download(attachment)
      let analysis = try await analyze(image)
      _ = try await client.completeAnalysisJob(
        job,
        lease: lease,
        image: image,
        result: analysis.encryptedResult
      )
      keepAlive.cancel()
      onNutritionReady?(analysis.mealID, analysis.desktopResult)
    } catch {
      keepAlive.cancel()
      let code = failureCode(for: error)
      _ = try? await client.failAnalysisJob(
        job,
        lease: lease,
        code: code,
        retryable: code == "model_unavailable" || code == "timeout"
      )
      throw error
    }
  }

  private func analyze(_ image: PremiumDecryptedHealthImage) async throws -> (
    mealID: String, desktopResult: String, encryptedResult: Data
  ) {
    guard let bridge else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let normalized = try PickerImageNormalizer.normalize(image.image)
    let imported = try blobImporter.importJPEG(normalized, appId: "health")
    let provider =
      ProcessInfo.processInfo.environment["TERRANE_HEALTH_VISION_PROVIDER"]
      ?? "opencode"
    let model =
      ProcessInfo.processInfo.environment["TERRANE_HEALTH_VISION_MODEL"]
      ?? "opencode-go/kimi-k2.6"
    let note = image.metadata.note ?? ""
    let result = try await bridge.invoke(
      appID: "health",
      verb: "estimate_blob",
      args: [imported.name, note, provider, model]
    )
    guard
      let data = result.data(using: .utf8),
      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["ok"] as? Bool == true,
      var estimate = object["estimate"] as? [String: Any],
      let mealID = estimate["id"] as? String
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }

    // Device-local identifiers and blob paths are not meaningful on iPhone.
    estimate.removeValue(forKey: "id")
    estimate.removeValue(forKey: "blob_name")
    estimate.removeValue(forKey: "source_job_id")
    let payload: [String: Any] = [
      "contract": premiumHealthAnalysisResultContract,
      "estimate": estimate,
      "provider": provider,
      "model": model,
      "note": note,
      "completed_at": ISO8601DateFormatter().string(from: Date()),
    ]
    let encryptedResult = try JSONSerialization.data(
      withJSONObject: payload,
      options: [.sortedKeys]
    )

    #if DEBUG
      if let resultPath = ProcessInfo.processInfo.environment[
        "TERRANE_E2E_HEALTH_RESULT_PATH"
      ], !resultPath.isEmpty {
        try? result.write(
          to: URL(fileURLWithPath: resultPath),
          atomically: true,
          encoding: .utf8
        )
      }
    #endif
    return (mealID, result, encryptedResult)
  }

  private func failureCode(for error: Error) -> String {
    if error is PremiumHealthImageSyncError {
      return "decrypt_failed"
    }
    let message = error.localizedDescription.lowercased()
    if message.contains("timed out") || message.contains("timeout") {
      return "timeout"
    }
    if message.contains("opencode") || message.contains("model") {
      return "model_unavailable"
    }
    return "analysis_failed"
  }

  private func record(_ error: Error) {
    let description = error.localizedDescription
    if description != lastErrorDescription {
      lastErrorDescription = description
      NSLog("terrane-host: Health analysis sync waiting: \(description)")
    }
    #if DEBUG
      if let errorPath = ProcessInfo.processInfo.environment[
        "TERRANE_E2E_HEALTH_ERROR_PATH"
      ], !errorPath.isEmpty {
        try? description.write(
          to: URL(fileURLWithPath: errorPath),
          atomically: true,
          encoding: .utf8
        )
      }
    #endif
  }
}
