import Foundation
import TerranePremiumSession

@MainActor
final class IOSAppStateAutoSync {
  private let client: PremiumAppStateSyncClient
  private let runtime: any TerraneRuntime
  private let defaults: UserDefaults
  private var task: Task<Void, Never>?

  var onStatus: ((String) -> Void)?

  init(
    client: PremiumAppStateSyncClient,
    runtime: any TerraneRuntime,
    defaults: UserDefaults = .standard
  ) {
    self.client = client
    self.runtime = runtime
    self.defaults = defaults
  }

  func start() {
    guard task == nil else { return }
    task = Task { [weak self] in
      guard let self else { return }
      while !Task.isCancelled {
        await syncAll()
        try? await Task.sleep(for: .seconds(5))
      }
    }
  }

  func stop() {
    task?.cancel()
    task = nil
  }

  private func syncAll() async {
    var changedApps: [String] = []
    do {
      let syncDeviceID = try await client.currentDeviceID()
      for appID in ["bmi-calculator", "health"] {
        if appID == "health" {
          _ = try? await runtime.invoke(appID: appID, verb: "sync_prepare", arguments: [])
        }
        let checkpointKey = "terrane.app-sync.version.\(syncDeviceID).\(appID)"
        let checkpoint = defaults.string(forKey: checkpointKey) ?? ""
        let localUpdate = try await runtime.crdtExport(
          appID: appID,
          sinceVersion: checkpoint
        )
        let digestKey = "terrane.app-sync.update-digest.\(syncDeviceID).\(appID)"
        let localDigest =
          localUpdate.isEmpty ? nil : premiumAppSyncUpdateID(localUpdate)
        let updateToUpload: Data? =
          localUpdate.isEmpty || localDigest == defaults.string(forKey: digestKey)
          ? nil : localUpdate
        let result = try await client.sync(
          appID: appID,
          localUpdate: updateToUpload
        )
        var changed = result.uploaded
        for update in result.updates {
          changed = try await runtime.crdtMerge(appID: appID, update: update.update) || changed
        }
        defaults.set(try await runtime.crdtVersion(appID: appID), forKey: checkpointKey)
        if let localDigest {
          defaults.set(localDigest, forKey: digestKey)
        }
        if changed { changedApps.append(appID == "health" ? "Health" : "BMI") }
      }
      onStatus?(
        changedApps.isEmpty
          ? "Mac and iPhone app data are up to date"
          : "Synced \(changedApps.joined(separator: " and ")) with Mac"
      )
    } catch {
      onStatus?("App sync paused: \(error.localizedDescription)")
    }
  }
}
