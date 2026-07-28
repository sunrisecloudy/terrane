import Foundation
import TerranePremiumSession
import UIKit

enum PremiumAccountViewState: Equatable {
  case signedOut
  case authenticating(PremiumIdentityProvider)
  case signedIn(PremiumAccount)
  case error(message: String)
}

@MainActor
final class PremiumAccountController: ObservableObject {
  @Published private(set) var state: PremiumAccountViewState = .signedOut

  let accountDeletionURL: URL

  private let client: PremiumSessionClient

  init?(baseURL: URL, keychainService: String, bundle: Bundle = .main) {
    let version =
      bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
      ?? "development"
    do {
      client = try PremiumSessionClient(
        baseURL: baseURL,
        device: PremiumDeviceMetadata(
          platform: .iOS,
          deviceName: UIDevice.current.name,
          clientVersion: version
        ),
        tokenStore: PremiumKeychainRefreshTokenStore(service: keychainService)
      )
    } catch {
      return nil
    }
    accountDeletionURL = baseURL.appendingPathComponent("settings/account/delete")
  }

  func restore() async {
    await client.restoreSession()
    await synchronizeState()
  }

  func begin(
    _ provider: PremiumIdentityProvider
  ) async throws -> PremiumAuthenticationChallenge {
    state = .authenticating(provider)
    do {
      return try await client.beginAuthentication(provider: provider)
    } catch {
      state = .error(message: error.localizedDescription)
      throw error
    }
  }

  func completeApple(_ credential: PremiumAppleCredential, challengeID: String) async {
    do {
      let account = try await client.exchangeAppleCredential(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      state = .error(message: error.localizedDescription)
    }
  }

  func completeGoogle(_ credential: PremiumGoogleCredential, challengeID: String) async {
    do {
      let account = try await client.exchangeGoogleCredential(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      state = .error(message: error.localizedDescription)
    }
  }

  func cancelAuthentication() async {
    await client.cancelAuthentication()
    await synchronizeState()
  }

  func authenticationFailed(_ error: Error) async {
    await client.cancelAuthentication()
    state = .error(message: error.localizedDescription)
  }

  func signOut() async {
    try? await client.logout()
    state = .signedOut
  }

  func dismissError() async {
    await synchronizeState()
  }

  private func synchronizeState() async {
    switch await client.state {
    case .signedOut, .revoked:
      state = .signedOut
    case .authenticating(let context):
      state = .authenticating(context.provider)
    case .signedIn(let account):
      state = .signedIn(account)
    case .refreshing(let account):
      state = account.map(PremiumAccountViewState.signedIn) ?? .signedOut
    case .offline(let context):
      if let account = context.account {
        state = .signedIn(account)
      } else {
        state = .error(message: context.message)
      }
    }
  }
}
