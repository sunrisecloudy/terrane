import Foundation
import TerranePremiumSession

@MainActor
final class MacAppStateAutoSyncCoordinator {
  private let client: PremiumAppStateSyncClient
  private let bridge: TerraneBridge
  private let defaults: UserDefaults
  private var task: Task<Void, Never>?

  var onStatus: ((String) -> Void)?

  init(
    session: PremiumSessionClient,
    bridge: TerraneBridge,
    defaults: UserDefaults = .standard
  ) {
    client = PremiumAppStateSyncClient(session: session, platform: .macOS)
    self.bridge = bridge
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
          _ = try? await bridge.invoke(appID: appID, verb: "sync_prepare", args: [])
        }
        let checkpointKey = "terrane.app-sync.version.\(syncDeviceID).\(appID)"
        let checkpoint = defaults.string(forKey: checkpointKey) ?? ""
        let localUpdate = try await bridge.crdtExport(
          appID: appID,
          sinceVersion: checkpoint
        )
        let result = try await client.sync(
          appID: appID,
          localUpdate: localUpdate.isEmpty ? nil : localUpdate
        )
        var changed = result.uploaded
        for update in result.updates {
          changed = try await bridge.crdtMerge(appID: appID, update: update.update) || changed
        }
        defaults.set(try await bridge.crdtVersion(appID: appID), forKey: checkpointKey)
        if changed { changedApps.append(appID == "health" ? "Health" : "BMI") }
      }
      onStatus?(
        changedApps.isEmpty
          ? "iPhone and Mac app data are up to date"
          : "Synced \(changedApps.joined(separator: " and ")) with iPhone"
      )
    } catch {
      onStatus?("App sync paused: \(error.localizedDescription)")
    }
  }
}
