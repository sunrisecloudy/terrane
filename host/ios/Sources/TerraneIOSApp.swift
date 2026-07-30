import GoogleSignIn
import SwiftUI

@main
struct TerraneIOSApp: App {
  @StateObject private var model = TerraneIOSModel(configuration: .current)

  var body: some Scene {
    WindowGroup {
      RootView(model: model)
        .onOpenURL { url in
          GIDSignIn.sharedInstance.handle(url)
        }
        .task {
          await model.start()
        }
    }
  }
}

@MainActor
final class TerraneIOSModel: ObservableObject {
  @Published private(set) var apps: [TerraneApp] = []
  @Published var selectedApp: TerraneApp?
  @Published private(set) var healthSyncStatus = ""

  let configuration: AppConfiguration
  let runtime: any TerraneRuntime
  let premiumAccount: PremiumAccountController?
  let healthAutoSync: IOSHealthAutoSync?

  init(configuration: AppConfiguration) {
    self.configuration = configuration
    runtime = TerraneRuntimeFactory.make()
    if let baseURL = configuration.premiumBaseURL {
      let account = PremiumAccountController(
        baseURL: baseURL,
        keychainService: "com.terrane.ios.premium-session",
        bundle: .main
      )
      premiumAccount = account
      if let account, let keyStore = HealthSyncKeyStoreFactory.make() {
        healthAutoSync = IOSHealthAutoSync(
          client: account.makeHealthImageSyncClient(
            keyStore: keyStore,
            deviceKeyStore: HealthSyncDeviceKeyStoreFactory.make()
          )
        )
      } else {
        healthAutoSync = nil
      }
    } else {
      premiumAccount = nil
      healthAutoSync = nil
    }
  }

  func start() async {
    apps = BundledAppCatalog.load()
    if let premiumAccount {
      await premiumAccount.restore()
    }
    await uploadE2EFixtureIfRequested()
  }

  private func uploadE2EFixtureIfRequested() async {
    #if DEBUG
      guard
        let fixture = ProcessInfo.processInfo.environment["TERRANE_E2E_HEALTH_FIXTURE"],
        !fixture.isEmpty,
        let healthAutoSync
      else { return }
      let name = (fixture as NSString).deletingPathExtension
      let fileExtension = (fixture as NSString).pathExtension
      guard
        let url =
          Bundle.main.url(
            forResource: name,
            withExtension: fileExtension,
            subdirectory: "FoodImages"
          )
          ?? Bundle.main.url(forResource: name, withExtension: fileExtension),
        let data = try? Data(contentsOf: url)
      else {
        healthSyncStatus = "Health fixture unavailable"
        return
      }
      do {
        let attachment = try await healthAutoSync.upload(data: data)
        healthSyncStatus = "Uploaded \(fixture) · \(attachment.id)"
      } catch {
        healthSyncStatus = "Health upload failed: \(error.localizedDescription)"
      }
    #endif
  }
}
