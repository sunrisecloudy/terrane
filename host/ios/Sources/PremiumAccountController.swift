import Foundation
import TerranePremiumSession
import UIKit

enum PremiumAccountViewState: Equatable {
  case signedOut
  case authenticating(PremiumIdentityProvider)
  case switching(PremiumIdentityProvider)
  case signedIn(PremiumAccount)
  case error(message: String)
}

@MainActor
final class PremiumAccountController: ObservableObject {
  @Published private(set) var state: PremiumAccountViewState = .signedOut

  let accountDeletionURL: URL

  private let client: PremiumSessionClient
  private let tokenStore: any PremiumRefreshTokenStore

  init?(baseURL: URL, keychainService: String, bundle: Bundle = .main) {
    let version =
      bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
      ?? "development"
    do {
      #if DEBUG
        if let refreshToken = ProcessInfo.processInfo.environment[
          "TERRANE_E2E_PREMIUM_REFRESH_TOKEN"
        ], !refreshToken.isEmpty {
          tokenStore = PremiumVolatileRefreshTokenStore(refreshToken: refreshToken)
        } else {
          tokenStore = PremiumKeychainRefreshTokenStore(service: keychainService)
        }
      #else
        tokenStore = PremiumKeychainRefreshTokenStore(service: keychainService)
      #endif
      client = try PremiumSessionClient(
        baseURL: baseURL,
        device: PremiumDeviceMetadata(
          platform: .iOS,
          deviceName: UIDevice.current.name,
          clientVersion: version
        ),
        tokenStore: tokenStore
      )
    } catch {
      return nil
    }
    accountDeletionURL = baseURL.appendingPathComponent("settings/account/delete")
  }

  func restore() async {
    #if DEBUG
      if let refreshToken = ProcessInfo.processInfo.environment[
        "TERRANE_E2E_PREMIUM_REFRESH_TOKEN"
      ], !refreshToken.isEmpty {
        try? await tokenStore.save(refreshToken)
      }
    #endif
    await client.restoreSession()
    await synchronizeState()
  }

  func makeHealthImageSyncClient(
    keyStore: any PremiumHealthImageKeyStore,
    deviceKeyStore: any PremiumHealthDeviceKeyStore =
      PremiumKeychainHealthDeviceKeyStore()
  ) -> PremiumHealthImageSyncClient {
    PremiumHealthImageSyncClient(
      session: client,
      keyStore: keyStore,
      deviceKeyStore: deviceKeyStore,
      platform: .iOS
    )
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

  func beginLink(
    _ provider: PremiumIdentityProvider
  ) async throws -> PremiumAuthenticationChallenge {
    state = .authenticating(provider)
    do {
      return try await client.beginLinkAuthentication(provider: provider)
    } catch {
      state = .error(message: error.localizedDescription)
      throw error
    }
  }

  func beginSwitch(
    _ provider: PremiumIdentityProvider
  ) async throws -> PremiumAuthenticationChallenge {
    state = .switching(provider)
    do {
      return try await client.beginAccountSwitch(provider: provider)
    } catch {
      await synchronizeState()
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

  func completeAppleLink(_ credential: PremiumAppleCredential, challengeID: String) async {
    do {
      let account = try await client.linkAppleCredential(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      state = .error(message: error.localizedDescription)
    }
  }

  func completeGoogleLink(_ credential: PremiumGoogleCredential, challengeID: String) async {
    do {
      let account = try await client.linkGoogleCredential(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      state = .error(message: error.localizedDescription)
    }
  }

  func completeAppleSwitch(_ credential: PremiumAppleCredential, challengeID: String) async {
    do {
      let account = try await client.switchAppleAccount(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      await synchronizeState()
      state = .error(message: error.localizedDescription)
    }
  }

  func completeGoogleSwitch(_ credential: PremiumGoogleCredential, challengeID: String) async {
    do {
      let account = try await client.switchGoogleAccount(
        challengeId: challengeID,
        credential: credential
      )
      state = .signedIn(account)
    } catch {
      await synchronizeState()
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
