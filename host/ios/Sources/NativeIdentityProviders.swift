import AuthenticationServices
import CryptoKit
import Foundation
import GoogleSignIn
import TerranePremiumSession
import UIKit

@MainActor
final class AppleIdentityProvider: NSObject, ASAuthorizationControllerDelegate,
  ASAuthorizationControllerPresentationContextProviding
{
  private var continuation: CheckedContinuation<PremiumAppleCredential, Error>?

  func signIn(
    challenge: PremiumAuthenticationChallenge
  ) async throws -> PremiumAppleCredential {
    let request = ASAuthorizationAppleIDProvider().createRequest()
    request.requestedScopes = [.fullName, .email]
    if let nonceSha256 = challenge.nonceSha256 {
      request.nonce = nonceSha256
    } else if let nonce = challenge.nonce {
      request.nonce = SHA256.hash(data: Data(nonce.utf8))
        .compactMap { String(format: "%02x", $0) }
        .joined()
    }
    return try await withCheckedThrowingContinuation { continuation in
      self.continuation = continuation
      let controller = ASAuthorizationController(authorizationRequests: [request])
      controller.delegate = self
      controller.presentationContextProvider = self
      controller.performRequests()
    }
  }

  func authorizationController(
    controller: ASAuthorizationController,
    didCompleteWithAuthorization authorization: ASAuthorization
  ) {
    guard let credential = authorization.credential as? ASAuthorizationAppleIDCredential,
      let tokenData = credential.identityToken,
      let codeData = credential.authorizationCode,
      let identityToken = String(data: tokenData, encoding: .utf8),
      let authorizationCode = String(data: codeData, encoding: .utf8)
    else {
      resume(
        throwing: TerraneIdentityError.missingCredential(
          "Apple did not return a usable identity credential."
        )
      )
      return
    }
    resume(
      returning: PremiumAppleCredential(
        identityToken: identityToken,
        authorizationCode: authorizationCode,
        displayName: PersonNameComponentsFormatter().string(
          from: credential.fullName ?? PersonNameComponents()
        ).nilIfEmpty
      )
    )
  }

  func authorizationController(
    controller: ASAuthorizationController,
    didCompleteWithError error: Error
  ) {
    resume(throwing: error)
  }

  func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
    UIApplication.shared.connectedScenes
      .compactMap { $0 as? UIWindowScene }
      .flatMap(\.windows)
      .first(where: \.isKeyWindow) ?? ASPresentationAnchor()
  }

  private func resume(returning credential: PremiumAppleCredential) {
    continuation?.resume(returning: credential)
    continuation = nil
  }

  private func resume(throwing error: Error) {
    continuation?.resume(throwing: error)
    continuation = nil
  }
}

@MainActor
final class GoogleIdentityProvider {
  func signIn(
    clientID: String,
    serverClientID: String?,
    challenge: PremiumAuthenticationChallenge
  ) async throws -> PremiumGoogleCredential {
    let nonce = try GoogleNativeChallengeNonce.require(from: challenge)
    GIDSignIn.sharedInstance.configuration = GIDConfiguration(
      clientID: clientID,
      serverClientID: serverClientID
    )
    guard
      let presenter = UIApplication.shared.connectedScenes
        .compactMap({ $0 as? UIWindowScene })
        .flatMap(\.windows)
        .first(where: \.isKeyWindow)?
        .rootViewController
    else {
      throw TerraneIdentityError.missingPresenter
    }
    let result = try await GIDSignIn.sharedInstance.signIn(
      withPresenting: presenter,
      hint: nil,
      additionalScopes: nil,
      nonce: nonce
    )
    guard let idToken = result.user.idToken?.tokenString, !idToken.isEmpty else {
      throw TerraneIdentityError.missingCredential(
        "Google did not return an ID token for Terrane Premium."
      )
    }
    return PremiumGoogleCredential(idToken: idToken)
  }

  func signOut() {
    GIDSignIn.sharedInstance.signOut()
  }
}

enum GoogleNativeChallengeNonce {
  static func require(from challenge: PremiumAuthenticationChallenge) throws -> String {
    guard challenge.provider == nil || challenge.provider == .google,
      let nonce = challenge.nonce?.trimmingCharacters(in: .whitespacesAndNewlines),
      !nonce.isEmpty
    else {
      throw TerraneIdentityError.missingCredential(
        "Premium did not return the Google nonce required for secure sign-in."
      )
    }
    return nonce
  }
}

extension String {
  fileprivate var nilIfEmpty: String? {
    isEmpty ? nil : self
  }
}

enum TerraneIdentityError: LocalizedError {
  case missingPresenter
  case missingCredential(String)

  var errorDescription: String? {
    switch self {
    case .missingPresenter:
      return "Terrane could not present the sign-in screen."
    case .missingCredential(let message):
      return message
    }
  }
}
