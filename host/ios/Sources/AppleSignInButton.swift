import AuthenticationServices
import SwiftUI

struct AppleSignInButton: UIViewRepresentable {
  let action: () -> Void

  func makeCoordinator() -> Coordinator {
    Coordinator(action: action)
  }

  func makeUIView(context: Context) -> ASAuthorizationAppleIDButton {
    let button = ASAuthorizationAppleIDButton(
      authorizationButtonType: .signIn,
      authorizationButtonStyle: .black
    )
    button.cornerRadius = 10
    button.addTarget(
      context.coordinator,
      action: #selector(Coordinator.activate),
      for: .touchUpInside
    )
    return button
  }

  func updateUIView(_ uiView: ASAuthorizationAppleIDButton, context: Context) {}

  final class Coordinator: NSObject {
    let action: () -> Void

    init(action: @escaping () -> Void) {
      self.action = action
    }

    @objc func activate() {
      action()
    }
  }
}
