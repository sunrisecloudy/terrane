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

  let configuration: AppConfiguration
  let runtime: any TerraneRuntime
  let premiumAccount: PremiumAccountController?

  init(configuration: AppConfiguration) {
    self.configuration = configuration
    runtime = TerraneRuntimeFactory.make()
    if let baseURL = configuration.premiumBaseURL {
      premiumAccount = PremiumAccountController(
        baseURL: baseURL,
        keychainService: "com.terrane.ios.premium-session",
        bundle: .main
      )
    } else {
      premiumAccount = nil
    }
  }

  func start() async {
    apps = BundledAppCatalog.load()
    if let premiumAccount {
      await premiumAccount.restore()
    }
  }
}
