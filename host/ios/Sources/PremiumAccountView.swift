import AuthenticationServices
import GoogleSignIn
import GoogleSignInSwift
import SwiftUI
import TerranePremiumSession

struct PremiumAccountView: View {
  let controller: PremiumAccountController?
  let configuration: AppConfiguration

  var body: some View {
    Group {
      if let controller {
        ConfiguredPremiumAccountView(
          controller: controller,
          configuration: configuration
        )
      } else {
        ContentUnavailableView {
          Label("Premium is not configured", systemImage: "person.crop.circle.badge.questionmark")
        } description: {
          Text("Local Terrane apps remain fully available. Add the Premium base URL to the iOS build configuration to enable sign-in.")
        }
        .accessibilityIdentifier("premium.not-configured")
      }
    }
    .navigationTitle("Account")
  }
}

private struct ConfiguredPremiumAccountView: View {
  @ObservedObject var controller: PremiumAccountController
  let configuration: AppConfiguration

  @State private var showSignOut = false
  @State private var showDelete = false
  private let apple = AppleIdentityProvider()
  private let google = GoogleIdentityProvider()
  @Environment(\.openURL) private var openURL

  var body: some View {
    Form {
      Section {
        stateContent
      } header: {
        Text("Terrane Premium")
      } footer: {
        Text("Premium is optional. Your local apps and data continue to work while signed out.")
      }

      if case .signedIn = controller.state {
        Section("Session") {
          Button("Sign out", role: .destructive) {
            showSignOut = true
          }
          .accessibilityIdentifier("premium.sign-out")
          Button("Delete Premium account", role: .destructive) {
            showDelete = true
          }
          .accessibilityIdentifier("premium.delete-account")
        }
      }

      Section("Privacy") {
        Label("Session credentials are stored in Keychain", systemImage: "key.fill")
        Label("Local apps never receive Premium tokens", systemImage: "hand.raised.fill")
      }
      .font(.callout)
    }
    .confirmationDialog(
      "Sign out of Terrane Premium?",
      isPresented: $showSignOut,
      titleVisibility: .visible
    ) {
      Button("Sign out", role: .destructive) {
        Task {
          google.signOut()
          await controller.signOut()
        }
      }
    } message: {
      Text("Local Terrane apps and data will stay on this device.")
    }
    .alert("Delete Premium account", isPresented: $showDelete) {
      Button("Cancel", role: .cancel) {}
      Button("Continue in secure browser", role: .destructive) {
        Task {
          google.signOut()
          await controller.signOut()
          openURL(controller.accountDeletionURL)
        }
      }
    } message: {
      Text("Terrane will sign out locally, then open the Premium account-deletion flow in the system browser. Local Terrane data is not deleted.")
    }
  }

  @ViewBuilder
  private var stateContent: some View {
    switch controller.state {
    case .signedOut:
      VStack(spacing: 12) {
        AppleSignInButton {
          authenticateWithApple()
        }
        .frame(height: 50)
        .accessibilityIdentifier("premium.sign-in.apple")

        GoogleSignInButton {
          authenticateWithGoogle()
        }
        .frame(height: 50)
        .disabled(configuration.googleClientID == nil)
        .accessibilityIdentifier("premium.sign-in.google")

        if configuration.googleClientID == nil {
          Text("Google Sign-In is not configured for this build.")
            .font(.caption)
            .foregroundStyle(.secondary)
        }
      }
      .padding(.vertical, 8)

    case .authenticating(let provider):
      HStack {
        ProgressView()
        Text("Signing in with \(provider == .apple ? "Apple" : "Google")…")
      }
      .accessibilityIdentifier("premium.authenticating")

    case .signedIn(let account):
      LabeledContent("Status", value: "Signed in")
      if let name = account.displayName {
        LabeledContent("Name", value: name)
      }
      if let email = account.email {
        LabeledContent("Email", value: email)
      }
    case .error(let message):
      Label(message, systemImage: "exclamationmark.triangle.fill")
        .foregroundStyle(.red)
        .accessibilityIdentifier("premium.error")
      Button("Try again") {
        Task {
          await controller.dismissError()
        }
      }
    }
  }

  private func authenticateWithApple() {
    Task {
      do {
        let challenge = try await controller.begin(.apple)
        let credential = try await apple.signIn(challenge: challenge)
        await controller.completeApple(credential, challengeID: challenge.challengeId)
      } catch let error as ASAuthorizationError where error.code == .canceled {
        await controller.cancelAuthentication()
      } catch {
        await controller.authenticationFailed(error)
      }
    }
  }

  private func authenticateWithGoogle() {
    guard let clientID = configuration.googleClientID else { return }
    Task {
      do {
        let challenge = try await controller.begin(.google)
        let credential = try await google.signIn(
          clientID: clientID,
          serverClientID: configuration.googleServerClientID
        )
        await controller.completeGoogle(credential, challengeID: challenge.challengeId)
      } catch {
        let nativeError = error as NSError
        if nativeError.domain == kGIDSignInErrorDomain,
           nativeError.code == -5 // kGIDSignInErrorCodeCanceled
        {
          await controller.cancelAuthentication()
        } else {
          await controller.authenticationFailed(error)
        }
      }
    }
  }
}
