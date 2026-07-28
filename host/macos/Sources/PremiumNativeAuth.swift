import AppKit
import AuthenticationServices
import CryptoKit
import GoogleSignIn
import GoogleSignInSwift
import SwiftUI
import TerranePremiumSession

enum PremiumNativeAuthError: LocalizedError, Equatable {
  case cancelled
  case notConfigured(String)
  case missingCredential(String)

  var errorDescription: String? {
    switch self {
    case .cancelled:
      return "Sign-in was cancelled."
    case .notConfigured(let message), .missingCredential(let message):
      return message
    }
  }
}

private enum PremiumNativeAuthorizationIntent {
  case signIn
  case link
}

struct PremiumNativeAuthConfiguration {
  let googleClientID: String?
  let googleServerClientID: String?

  static func fromBundle(_ bundle: Bundle = .main) -> PremiumNativeAuthConfiguration {
    PremiumNativeAuthConfiguration(
      googleClientID: configuredString(bundle.object(forInfoDictionaryKey: "GIDClientID")),
      googleServerClientID: configuredString(
        bundle.object(forInfoDictionaryKey: "GIDServerClientID"))
    )
  }

  private static func configuredString(_ value: Any?) -> String? {
    guard let value = value as? String else { return nil }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, !trimmed.hasPrefix("$(") else { return nil }
    return trimmed
  }
}

/// Owns the AuthenticationServices and GoogleSignIn SDK lifecycles. Provider
/// credentials are handed straight to the shared PremiumSessionClient.
/// Google is explicitly signed out after capture so its OAuth credentials do
/// not remain in the SDK Keychain; only Terrane's SaaS refresh token persists.
final class PremiumNativeAuthCoordinator: NSObject,
  ASAuthorizationControllerDelegate,
  ASAuthorizationControllerPresentationContextProviding
{
  var onCompletion: ((Result<PremiumAccount, Error>) -> Void)?

  private weak var window: NSWindow?
  private let session: PremiumSessionClient
  private let configuration: PremiumNativeAuthConfiguration
  private var pendingApple:
    (challenge: PremiumAuthenticationChallenge, intent: PremiumNativeAuthorizationIntent)?

  init(
    window: NSWindow,
    session: PremiumSessionClient,
    configuration: PremiumNativeAuthConfiguration = .fromBundle()
  ) {
    self.window = window
    self.session = session
    self.configuration = configuration
  }

  func signIn(with provider: PremiumIdentityProvider) {
    authorize(provider: provider, intent: .signIn)
  }

  func link(_ provider: PremiumIdentityProvider) {
    authorize(provider: provider, intent: .link)
  }

  private func authorize(
    provider: PremiumIdentityProvider,
    intent: PremiumNativeAuthorizationIntent
  ) {
    Task { [weak self] in
      guard let self else { return }
      do {
        let challenge = try await session.beginAuthentication(provider: provider)
        await MainActor.run {
          switch provider {
          case .apple:
            self.beginApple(challenge: challenge, intent: intent)
          case .google:
            self.beginGoogle(challenge: challenge, intent: intent)
          }
        }
      } catch {
        await MainActor.run {
          self.onCompletion?(.failure(error))
        }
      }
    }
  }

  private func beginApple(
    challenge: PremiumAuthenticationChallenge,
    intent: PremiumNativeAuthorizationIntent
  ) {
    pendingApple = (challenge, intent)
    let request = ASAuthorizationAppleIDProvider().createRequest()
    request.requestedScopes = [.fullName, .email]
    if let nonce = challenge.nonceSha256
      ?? challenge.nonce.map(Self.sha256)
    {
      request.nonce = nonce
    }
    let controller = ASAuthorizationController(authorizationRequests: [request])
    controller.delegate = self
    controller.presentationContextProvider = self
    controller.performRequests()
  }

  private func beginGoogle(
    challenge: PremiumAuthenticationChallenge,
    intent: PremiumNativeAuthorizationIntent
  ) {
    guard let clientID = configuration.googleClientID else {
      failConfiguration("Google Sign-In is not configured for this build.")
      return
    }
    guard let window else {
      failConfiguration("The sign-in window is unavailable.")
      return
    }
    GIDSignIn.sharedInstance.configuration = GIDConfiguration(
      clientID: clientID,
      serverClientID: configuration.googleServerClientID
    )
    GIDSignIn.sharedInstance.signIn(
      withPresenting: window,
      hint: nil,
      additionalScopes: nil,
      nonce: challenge.nonce
    ) { [weak self] result, error in
      guard let self else { return }
      if let error {
        GIDSignIn.sharedInstance.signOut()
        Task { await self.session.cancelAuthentication() }
        self.onCompletion?(.failure(error))
        return
      }
      guard let result, let idToken = result.user.idToken?.tokenString else {
        GIDSignIn.sharedInstance.signOut()
        self.failCredential("Google did not return an ID token for backend authentication.")
        return
      }
      GIDSignIn.sharedInstance.signOut()
      Task { [weak self] in
        guard let self else { return }
        do {
          let credential = PremiumGoogleCredential(idToken: idToken)
          let account: PremiumAccount
          switch intent {
          case .signIn:
            account = try await session.exchangeGoogleCredential(
              challengeId: challenge.challengeId, credential: credential)
          case .link:
            account = try await session.linkGoogleCredential(
              challengeId: challenge.challengeId, credential: credential)
          }
          await MainActor.run { self.onCompletion?(.success(account)) }
        } catch {
          await MainActor.run { self.onCompletion?(.failure(error)) }
        }
      }
    }
  }

  func authorizationController(
    controller: ASAuthorizationController,
    didCompleteWithAuthorization authorization: ASAuthorization
  ) {
    guard let pendingApple else {
      failCredential("Apple authorization completed without an active challenge.")
      return
    }
    self.pendingApple = nil
    guard let credential = authorization.credential as? ASAuthorizationAppleIDCredential,
      let tokenData = credential.identityToken,
      let identityToken = String(data: tokenData, encoding: .utf8),
      let codeData = credential.authorizationCode,
      let authorizationCode = String(data: codeData, encoding: .utf8)
    else {
      failCredential("Apple did not return the credentials required for backend authentication.")
      return
    }
    let displayName = credential.fullName.flatMap {
      PersonNameComponentsFormatter().string(from: $0)
    }.flatMap { $0.isEmpty ? nil : $0 }
    Task { [weak self] in
      guard let self else { return }
      do {
        let appleCredential = PremiumAppleCredential(
          identityToken: identityToken,
          authorizationCode: authorizationCode,
          displayName: displayName
        )
        let account: PremiumAccount
        switch pendingApple.intent {
        case .signIn:
          account = try await session.exchangeAppleCredential(
            challengeId: pendingApple.challenge.challengeId,
            credential: appleCredential
          )
        case .link:
          account = try await session.linkAppleCredential(
            challengeId: pendingApple.challenge.challengeId,
            credential: appleCredential
          )
        }
        await MainActor.run { self.onCompletion?(.success(account)) }
      } catch {
        await MainActor.run { self.onCompletion?(.failure(error)) }
      }
    }
  }

  func authorizationController(
    controller: ASAuthorizationController,
    didCompleteWithError error: Error
  ) {
    pendingApple = nil
    Task { await session.cancelAuthentication() }
    let authorizationError = error as? ASAuthorizationError
    if authorizationError?.code == .canceled {
      onCompletion?(.failure(PremiumNativeAuthError.cancelled))
    } else {
      onCompletion?(.failure(error))
    }
  }

  func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
    window ?? NSWindow()
  }

  private func failConfiguration(_ message: String) {
    Task { await session.cancelAuthentication() }
    onCompletion?(.failure(PremiumNativeAuthError.notConfigured(message)))
  }

  private func failCredential(_ message: String) {
    Task { await session.cancelAuthentication() }
    onCompletion?(.failure(PremiumNativeAuthError.missingCredential(message)))
  }

  private static func sha256(_ value: String) -> String {
    SHA256.hash(data: Data(value.utf8)).map { String(format: "%02x", $0) }.joined()
  }
}

private struct PremiumGoogleSignInButton: View {
  let action: () -> Void

  var body: some View {
    GoogleSignInButton(action: action)
      .frame(width: 260, height: 42)
  }
}

/// A small host-owned provider chooser. It is an AppKit sheet; the supported
/// Google SwiftUI control is hosted inside it and no web content participates.
final class PremiumSignInSheetController: NSObject {
  private weak var parent: NSWindow?
  private let panel: NSPanel
  private let onProvider: (PremiumIdentityProvider) -> Void

  init(
    parent: NSWindow,
    onProvider: @escaping (PremiumIdentityProvider) -> Void
  ) {
    self.parent = parent
    self.onProvider = onProvider
    panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 360, height: 250),
      styleMask: [.titled],
      backing: .buffered,
      defer: false
    )
    super.init()
    configure()
  }

  func present() {
    guard let parent, panel.sheetParent == nil else { return }
    parent.beginSheet(panel)
  }

  func dismiss() {
    if let parent, panel.sheetParent != nil {
      parent.endSheet(panel)
    }
  }

  var panelForTesting: NSPanel { panel }

  private func configure() {
    panel.title = "Terrane Premium"
    panel.isMovable = false

    let content = NSView()
    let title = NSTextField(labelWithString: "Sign in to Terrane Premium")
    title.font = .systemFont(ofSize: 18, weight: .semibold)
    title.alignment = .center
    title.translatesAutoresizingMaskIntoConstraints = false

    let detail = NSTextField(
      wrappingLabelWithString:
        "Sync and Premium services are optional. Terrane stays fully usable offline without an account."
    )
    detail.textColor = .secondaryLabelColor
    detail.alignment = .center
    detail.translatesAutoresizingMaskIntoConstraints = false

    let appleButton = ASAuthorizationAppleIDButton(
      authorizationButtonType: .signIn,
      authorizationButtonStyle: .black
    )
    appleButton.target = self
    appleButton.action = #selector(appleClicked)
    appleButton.translatesAutoresizingMaskIntoConstraints = false

    let googleButton = NSHostingView(
      rootView: PremiumGoogleSignInButton { [weak self] in self?.googleClicked() }
    )
    googleButton.translatesAutoresizingMaskIntoConstraints = false

    let cancel = NSButton(title: "Not now", target: self, action: #selector(cancelClicked))
    cancel.bezelStyle = .inline
    cancel.translatesAutoresizingMaskIntoConstraints = false

    [title, detail, appleButton, googleButton, cancel].forEach(content.addSubview)
    panel.contentView = content
    NSLayoutConstraint.activate([
      title.topAnchor.constraint(equalTo: content.topAnchor, constant: 22),
      title.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24),
      title.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24),

      detail.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 8),
      detail.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 30),
      detail.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -30),

      appleButton.topAnchor.constraint(equalTo: detail.bottomAnchor, constant: 18),
      appleButton.centerXAnchor.constraint(equalTo: content.centerXAnchor),
      appleButton.widthAnchor.constraint(equalToConstant: 260),
      appleButton.heightAnchor.constraint(equalToConstant: 42),

      googleButton.topAnchor.constraint(equalTo: appleButton.bottomAnchor, constant: 10),
      googleButton.centerXAnchor.constraint(equalTo: content.centerXAnchor),
      googleButton.widthAnchor.constraint(equalToConstant: 260),
      googleButton.heightAnchor.constraint(equalToConstant: 42),

      cancel.topAnchor.constraint(equalTo: googleButton.bottomAnchor, constant: 8),
      cancel.centerXAnchor.constraint(equalTo: content.centerXAnchor),
    ])
  }

  @objc private func appleClicked() {
    dismiss()
    onProvider(.apple)
  }

  private func googleClicked() {
    dismiss()
    onProvider(.google)
  }

  @objc private func cancelClicked() {
    dismiss()
  }
}
