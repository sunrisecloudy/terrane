import AuthenticationServices
import GoogleSignIn
import GoogleSignInSwift
import SwiftUI
import TerranePremiumSession

struct PremiumAccountView: View {
  let controller: PremiumAccountController?
  let configuration: AppConfiguration
  let healthAutoSync: IOSHealthAutoSync?

  var body: some View {
    Group {
      if let controller {
        ConfiguredPremiumAccountView(
          controller: controller,
          configuration: configuration,
          healthAutoSync: healthAutoSync
        )
      } else {
        ContentUnavailableView {
          Label("Premium is not configured", systemImage: "person.crop.circle.badge.questionmark")
        } description: {
          Text(
            "Local Terrane apps remain fully available. Add the Premium base URL to the iOS build configuration to enable sign-in."
          )
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
  let healthAutoSync: IOSHealthAutoSync?

  @State private var showSignOut = false
  @State private var showDelete = false
  @State private var showAccountSwitch = false
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
        linkedProviderSection
        if let healthAutoSync {
          HealthConnectionSection(sync: healthAutoSync)
        }

        Section("Session") {
          Button("Switch account") {
            showAccountSwitch = true
          }
          .accessibilityIdentifier("premium.switch-account")
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
      "Switch Terrane Premium account?",
      isPresented: $showAccountSwitch,
      titleVisibility: .visible
    ) {
      Button("Continue with Apple") {
        switchWithApple()
      }
      Button("Continue with Google") {
        switchWithGoogle()
      }
      .disabled(configuration.googleClientID == nil)
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        "Your current account stays signed in if you cancel or the new sign-in fails. Local Terrane apps and data stay on this device."
      )
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
      Text(
        "Terrane will sign out locally, then open the Premium account-deletion flow in the system browser. Local Terrane data is not deleted."
      )
    }
  }

  @ViewBuilder
  private var linkedProviderSection: some View {
    if case .signedIn(let account) = controller.state {
      Section("Linked sign-in") {
        ForEach(account.linkedProviders, id: \.rawValue) { provider in
          Label(
            provider == .apple ? "Apple linked" : "Google linked",
            systemImage: "checkmark.circle.fill"
          )
        }

        if !account.linkedProviders.contains(.apple) {
          Button("Link Apple") {
            linkWithApple()
          }
          .accessibilityIdentifier("premium.link.apple")
        }
        if !account.linkedProviders.contains(.google) {
          Button("Link Google") {
            linkWithGoogle()
          }
          .disabled(configuration.googleClientID == nil)
          .accessibilityIdentifier("premium.link.google")
        }
      }
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

    case .switching(let provider):
      HStack {
        ProgressView()
        Text("Switching with \(provider == .apple ? "Apple" : "Google")…")
      }
      .accessibilityIdentifier("premium.switching")

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
          serverClientID: configuration.googleServerClientID,
          challenge: challenge
        )
        await controller.completeGoogle(credential, challengeID: challenge.challengeId)
      } catch {
        let nativeError = error as NSError
        if nativeError.domain == kGIDSignInErrorDomain,
          nativeError.code == -5  // kGIDSignInErrorCodeCanceled
        {
          await controller.cancelAuthentication()
        } else {
          await controller.authenticationFailed(error)
        }
      }
    }
  }

  private func linkWithApple() {
    Task {
      do {
        let challenge = try await controller.beginLink(.apple)
        let credential = try await apple.signIn(challenge: challenge)
        await controller.completeAppleLink(credential, challengeID: challenge.challengeId)
      } catch let error as ASAuthorizationError where error.code == .canceled {
        await controller.cancelAuthentication()
      } catch {
        await controller.authenticationFailed(error)
      }
    }
  }

  private func linkWithGoogle() {
    guard let clientID = configuration.googleClientID else { return }
    Task {
      do {
        let challenge = try await controller.beginLink(.google)
        let credential = try await google.signIn(
          clientID: clientID,
          serverClientID: configuration.googleServerClientID,
          challenge: challenge
        )
        await controller.completeGoogleLink(credential, challengeID: challenge.challengeId)
      } catch {
        let nativeError = error as NSError
        if nativeError.domain == kGIDSignInErrorDomain,
          nativeError.code == -5
        {
          await controller.cancelAuthentication()
        } else {
          await controller.authenticationFailed(error)
        }
      }
    }
  }

  private func switchWithApple() {
    Task {
      do {
        let challenge = try await controller.beginSwitch(.apple)
        let credential = try await apple.signIn(challenge: challenge)
        await controller.completeAppleSwitch(credential, challengeID: challenge.challengeId)
      } catch let error as ASAuthorizationError where error.code == .canceled {
        await controller.cancelAuthentication()
      } catch {
        await controller.authenticationFailed(error)
      }
    }
  }

  private func switchWithGoogle() {
    guard let clientID = configuration.googleClientID else { return }
    google.signOut()
    Task {
      do {
        let challenge = try await controller.beginSwitch(.google)
        let credential = try await google.signIn(
          clientID: clientID,
          serverClientID: configuration.googleServerClientID,
          challenge: challenge
        )
        await controller.completeGoogleSwitch(credential, challengeID: challenge.challengeId)
      } catch {
        let nativeError = error as NSError
        if nativeError.domain == kGIDSignInErrorDomain,
          nativeError.code == -5
        {
          await controller.cancelAuthentication()
        } else {
          await controller.authenticationFailed(error)
        }
      }
    }
  }
}

private struct HealthConnectionSection: View {
  @ObservedObject var sync: IOSHealthAutoSync

  private var macs: [PremiumHealthConnectionDevice] {
    (sync.connections?.devices ?? []).filter { $0.platform == "macos" }
  }

  var body: some View {
    Section {
      if macs.isEmpty {
        Label("No Mac has joined this account yet", systemImage: "desktopcomputer")
          .foregroundStyle(.secondary)
      } else {
        ForEach(macs) { mac in
          let sendsToMac =
            sync.connections?.pairings.contains {
              $0.senderDeviceId == sync.currentDeviceID
                && $0.recipientDeviceId == mac.deviceId
                && $0.status == "approved"
            } == true
          let receivesFromMac =
            sync.connections?.pairings.contains {
              $0.senderDeviceId == mac.deviceId
                && $0.recipientDeviceId == sync.currentDeviceID
                && $0.status == "approved"
            } == true
          LabeledContent {
            Text(
              connectionLabel(
                sendsToMac: sendsToMac,
                receivesFromMac: receivesFromMac,
                mac: mac
              )
            )
            .foregroundStyle(
              connectionColor(
                sendsToMac: sendsToMac,
                receivesFromMac: receivesFromMac,
                mac: mac
              ))
          } label: {
            Label(mac.name, systemImage: "desktopcomputer")
          }
          .accessibilityIdentifier("health.connection.\(mac.deviceId)")
          if !receivesFromMac {
            Button("Allow sync from \(mac.name)") {
              Task { await sync.approveMac(deviceID: mac.deviceId) }
            }
          }
        }
      }
      if !sync.connectionsError.isEmpty {
        Text(sync.connectionsError)
          .font(.caption)
          .foregroundStyle(.red)
      }
      Button("Refresh connected devices") {
        Task { await sync.refreshConnections() }
      }
    } header: {
      Text("Health connection")
    } footer: {
      Text(
        "Your Mac decrypts food photos and analyzes nutrition locally. Premium stores only encrypted data."
      )
    }
    .task {
      await sync.refreshConnections()
    }
  }

  private func connectionLabel(
    sendsToMac: Bool,
    receivesFromMac: Bool,
    mac: PremiumHealthConnectionDevice
  ) -> String {
    guard sendsToMac else { return "Approve iPhone on Mac" }
    guard receivesFromMac else { return "Allow Mac on iPhone" }
    return mac.analyzer?.ready == true ? "Connected" : "Paired · Mac offline"
  }

  private func connectionColor(
    sendsToMac: Bool,
    receivesFromMac: Bool,
    mac: PremiumHealthConnectionDevice
  ) -> Color {
    sendsToMac && receivesFromMac && mac.analyzer?.ready == true ? .green : .secondary
  }
}
