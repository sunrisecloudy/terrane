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
  @Published private(set) var pinnedAppIDs: Set<String>
  @Published var selectedApp: TerraneApp?
  @Published private(set) var healthSyncStatus = ""
  @Published private(set) var startupError = ""
  @Published var permissionPrompt: IOSPermissionPrompt?

  let configuration: AppConfiguration
  let runtime: any TerraneRuntime
  let premiumAccount: PremiumAccountController?
  let healthAutoSync: IOSHealthAutoSync?
  private let appPreferences: UserDefaults
  private var permissionContinuation: CheckedContinuation<Bool, Never>?

  init(configuration: AppConfiguration, appPreferences: UserDefaults = .standard) {
    self.configuration = configuration
    self.appPreferences = appPreferences
    pinnedAppIDs = AppPinPreferences.load(from: appPreferences)
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

  var orderedApps: [TerraneApp] {
    AppPinPreferences.ordered(apps, pinnedAppIDs: pinnedAppIDs)
  }

  func isPinned(_ appID: String) -> Bool {
    pinnedAppIDs.contains(appID)
  }

  func togglePinned(_ appID: String) {
    if pinnedAppIDs.contains(appID) {
      pinnedAppIDs.remove(appID)
    } else {
      pinnedAppIDs.insert(appID)
    }
    AppPinPreferences.save(pinnedAppIDs, to: appPreferences)
  }

  func start() async {
    let bundledApps = BundledAppCatalog.load()
    var catalogErrors: [String] = []
    for app in bundledApps {
      do {
        _ = try await runtime.dispatch(
          command: "app.add",
          arguments: app.registrationArguments
        )
      } catch {
        catalogErrors.append("\(app.name): \(error.localizedDescription)")
      }
    }
    startupError = catalogErrors.joined(separator: "\n")
    apps = bundledApps
    if let premiumAccount {
      await premiumAccount.restore()
    }
    await uploadE2EFixtureIfRequested()
  }

  func requestPermission(app: TerraneApp, resources: [String]) async -> Bool {
    guard permissionContinuation == nil else { return false }
    return await withCheckedContinuation { continuation in
      permissionContinuation = continuation
      permissionPrompt = IOSPermissionPrompt(
        appID: app.id,
        appName: app.name,
        resources: resources
      )
    }
  }

  func resolvePermission(approved: Bool) {
    let continuation = permissionContinuation
    permissionContinuation = nil
    permissionPrompt = nil
    continuation?.resume(returning: approved)
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

struct IOSPermissionPrompt: Identifiable {
  let appID: String
  let appName: String
  let resources: [String]

  var id: String {
    "\(appID):\(resources.joined(separator: ","))"
  }
}
