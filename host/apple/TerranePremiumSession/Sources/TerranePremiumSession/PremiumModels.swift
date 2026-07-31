import Foundation

enum PremiumDateCoding {
  static let decode: @Sendable (Decoder) throws -> Date = { decoder in
    let container = try decoder.singleValueContainer()
    let value = try container.decode(String.self)
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = fractional.date(from: value) {
      return date
    }
    let standard = ISO8601DateFormatter()
    standard.formatOptions = [.withInternetDateTime]
    if let date = standard.date(from: value) {
      return date
    }
    throw DecodingError.dataCorruptedError(
      in: container,
      debugDescription: "Expected an ISO-8601 timestamp"
    )
  }
}

public enum PremiumIdentityProvider: String, Codable, CaseIterable, Sendable {
  case apple
  case google
}

public enum PremiumPlatform: String, Codable, Sendable {
  case iOS = "ios"
  case macOS = "macos"
}

public struct PremiumDeviceMetadata: Codable, Equatable, Sendable {
  public let platform: PremiumPlatform
  public let deviceName: String
  public let clientVersion: String

  public init(platform: PremiumPlatform, deviceName: String, clientVersion: String) {
    self.platform = platform
    self.deviceName = deviceName
    self.clientVersion = clientVersion
  }
}

public struct PremiumAccount: Codable, Equatable, Sendable {
  public let id: String
  public let email: String?
  public let displayName: String?
  public let linkedProviders: [PremiumIdentityProvider]

  public init(
    id: String,
    email: String? = nil,
    displayName: String? = nil,
    linkedProviders: [PremiumIdentityProvider] = []
  ) {
    self.id = id
    self.email = email
    self.displayName = displayName
    self.linkedProviders = linkedProviders
  }

  private enum CodingKeys: String, CodingKey {
    case id
    case email
    case displayName
    case linkedProviders
  }

  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    id = try container.decode(String.self, forKey: .id)
    email = try container.decodeIfPresent(String.self, forKey: .email)
    displayName = try container.decodeIfPresent(String.self, forKey: .displayName)
    linkedProviders =
      try container.decodeIfPresent([PremiumIdentityProvider].self, forKey: .linkedProviders) ?? []
  }
}

public struct PremiumAuthenticationChallenge: Codable, Equatable, Sendable {
  public let challengeId: String
  public let provider: PremiumIdentityProvider?
  /// Raw nonce returned by Premium. Hosts should retain it only for the active
  /// provider flow and use `nonceSha256` for Sign in with Apple.
  public let nonce: String?
  public let nonceSha256: String?
  public let expiresAt: Date?

  public init(
    challengeId: String,
    provider: PremiumIdentityProvider? = nil,
    nonce: String? = nil,
    nonceSha256: String? = nil,
    expiresAt: Date? = nil
  ) {
    self.challengeId = challengeId
    self.provider = provider
    self.nonce = nonce
    self.nonceSha256 = nonceSha256
    self.expiresAt = expiresAt
  }
}

public struct PremiumAuthenticationContext: Equatable, Sendable {
  public let provider: PremiumIdentityProvider
  public let challengeId: String?

  public init(provider: PremiumIdentityProvider, challengeId: String? = nil) {
    self.provider = provider
    self.challengeId = challengeId
  }
}

public struct PremiumOfflineContext: Equatable, Sendable {
  public let account: PremiumAccount?
  public let message: String

  public init(account: PremiumAccount?, message: String) {
    self.account = account
    self.message = message
  }
}

public enum PremiumSessionState: Equatable, Sendable {
  case signedOut
  case authenticating(PremiumAuthenticationContext)
  case signedIn(PremiumAccount)
  case refreshing(PremiumAccount?)
  case offline(PremiumOfflineContext)
  case revoked
}

public struct PremiumAppleCredential: Equatable, Sendable {
  public let identityToken: String
  public let authorizationCode: String
  public let displayName: String?

  public init(identityToken: String, authorizationCode: String, displayName: String? = nil) {
    self.identityToken = identityToken
    self.authorizationCode = authorizationCode
    self.displayName = displayName
  }
}

public struct PremiumGoogleCredential: Equatable, Sendable {
  public let idToken: String

  public init(idToken: String) {
    self.idToken = idToken
  }
}

public enum PremiumSessionError: Error, Equatable, Sendable {
  case invalidBaseURL
  case invalidPath
  case invalidResponse
  case server(statusCode: Int, message: String?)
  case transport(String)
  case missingRefreshToken
  case notAuthenticated
  case authenticationInProgress
  case accountAlreadySignedIn
  case accountSwitchInProgress
  case accountSwitchNotInProgress
}

extension PremiumSessionError: LocalizedError {
  public var errorDescription: String? {
    switch self {
    case .invalidBaseURL:
      return "The Premium API base URL is invalid."
    case .invalidPath:
      return "The Premium API path must be relative to the configured server."
    case .invalidResponse:
      return "The Premium server returned an invalid response."
    case .server(let statusCode, let message):
      return message ?? "The Premium server returned HTTP \(statusCode)."
    case .transport(let message):
      return message
    case .missingRefreshToken:
      return "No Premium refresh token is available."
    case .notAuthenticated:
      return "A Premium account is not signed in."
    case .authenticationInProgress:
      return "A different Premium authentication flow is already in progress."
    case .accountAlreadySignedIn:
      return "A Premium account is already signed in. Use Switch account instead."
    case .accountSwitchInProgress:
      return "Complete the active account switch instead of ordinary sign-in."
    case .accountSwitchNotInProgress:
      return "No Premium account switch is in progress."
    }
  }
}
