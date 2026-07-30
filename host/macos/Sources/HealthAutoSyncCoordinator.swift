import Foundation
import TerranePremiumSession

final class MacHealthAutoSyncCoordinator {
  private let client: PremiumHealthImageSyncClient
  private let blobImporter: TerraneBlobImporter
  private weak var bridge: TerraneBridge?
  private let processedDefaultsKey = "health.sync.processed-attachments.v1"
  private var task: Task<Void, Never>?
  private var lastErrorDescription: String?

  var onNutritionReady: ((String, String) -> Void)?

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
      if
        let encoded = ProcessInfo.processInfo.environment[
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
  }

  private func pollOnce() async {
    do {
      try await client.approveSenders()
      let processed = processedIDs()
      let attachments = try await client.list()
      lastErrorDescription = nil
      for attachment in attachments where !processed.contains(attachment.id) {
        if Task.isCancelled { return }
        let image = try await client.download(attachment)
        try await analyze(image)
      }
    } catch {
      // Offline, not-yet-granted, and signed-out states all retry on the next
      // bounded poll without blocking local Health use.
      let description = error.localizedDescription
      if description != lastErrorDescription {
        lastErrorDescription = description
        NSLog("terrane-host: Health auto-sync waiting: \(description)")
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

  private func analyze(_ image: PremiumDecryptedHealthImage) async throws {
    guard let bridge else { return }
    let normalized = try PickerImageNormalizer.normalize(image.image)
    let imported = try blobImporter.importJPEG(normalized, appId: "health")
    let provider = ProcessInfo.processInfo.environment["TERRANE_HEALTH_VISION_PROVIDER"]
      ?? "claude"
    let model = ProcessInfo.processInfo.environment["TERRANE_HEALTH_VISION_MODEL"]
      ?? ""
    let result = try await bridge.invoke(
      appID: "health",
      verb: "estimate_blob",
      args: [
        imported.name,
        "Automatically imported from iPhone Health sync.",
        provider,
        model,
      ]
    )
    guard
      let data = result.data(using: .utf8),
      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["ok"] as? Bool == true,
      let estimate = object["estimate"] as? [String: Any],
      let mealID = estimate["id"] as? String
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    _ = try await bridge.invoke(
      appID: "health",
      verb: "common.receive",
      args: [
        "link",
        #"{"route":"meal","segments":["\#(mealID)"],"params":{}}"#,
      ]
    )
    markProcessed(image.attachment.id)
    if let resultPath = ProcessInfo.processInfo.environment[
      "TERRANE_E2E_HEALTH_RESULT_PATH"
    ], !resultPath.isEmpty {
      try? result.write(
        to: URL(fileURLWithPath: resultPath),
        atomically: true,
        encoding: .utf8
      )
    }
    await MainActor.run {
      self.onNutritionReady?(mealID, result)
    }
  }

  private func processedIDs() -> Set<String> {
    Set(UserDefaults.standard.stringArray(forKey: processedDefaultsKey) ?? [])
  }

  private func markProcessed(_ id: String) {
    var values = processedIDs()
    values.insert(id)
    UserDefaults.standard.set(Array(values).sorted(), forKey: processedDefaultsKey)
  }
}
