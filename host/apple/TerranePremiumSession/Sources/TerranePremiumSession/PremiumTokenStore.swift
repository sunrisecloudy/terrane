import Foundation
import Security

public protocol PremiumRefreshTokenStore: Sendable {
  func read() async throws -> String?
  func save(_ refreshToken: String) async throws
  func delete() async throws
}

/// Process-local token storage for tests and unsigned development hosts.
/// Production applications should continue to use `PremiumKeychainRefreshTokenStore`.
public actor PremiumVolatileRefreshTokenStore: PremiumRefreshTokenStore {
  private var refreshToken: String?

  public init(refreshToken: String? = nil) {
    self.refreshToken = refreshToken
  }

  public func read() async throws -> String? {
    refreshToken
  }

  public func save(_ refreshToken: String) async throws {
    self.refreshToken = refreshToken
  }

  public func delete() async throws {
    refreshToken = nil
  }
}

public enum PremiumKeychainError: Error, Equatable, Sendable {
  case unexpectedStatus(OSStatus)
  case invalidData
}

public struct PremiumKeychainRefreshTokenStore: PremiumRefreshTokenStore, Sendable {
  private let service: String
  private let account: String
  private let accessGroup: String?

  public init(
    service: String = "com.terrane.premium.session",
    account: String = "refresh-token",
    accessGroup: String? = nil
  ) {
    self.service = service
    self.account = account
    self.accessGroup = accessGroup
  }

  public func read() async throws -> String? {
    var query = baseQuery
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne

    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound {
      return nil
    }
    guard status == errSecSuccess else {
      throw PremiumKeychainError.unexpectedStatus(status)
    }
    guard let data = item as? Data, let token = String(data: data, encoding: .utf8) else {
      throw PremiumKeychainError.invalidData
    }
    return token
  }

  public func save(_ refreshToken: String) async throws {
    guard let data = refreshToken.data(using: .utf8) else {
      throw PremiumKeychainError.invalidData
    }
    var attributes = baseQuery
    attributes[kSecValueData as String] = data
    attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly

    let addStatus = SecItemAdd(attributes as CFDictionary, nil)
    if addStatus == errSecDuplicateItem {
      let update: [String: Any] = [
        kSecValueData as String: data,
        kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
      ]
      let status = SecItemUpdate(baseQuery as CFDictionary, update as CFDictionary)
      guard status == errSecSuccess else {
        throw PremiumKeychainError.unexpectedStatus(status)
      }
      return
    }
    guard addStatus == errSecSuccess else {
      throw PremiumKeychainError.unexpectedStatus(addStatus)
    }
  }

  public func delete() async throws {
    let status = SecItemDelete(baseQuery as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw PremiumKeychainError.unexpectedStatus(status)
    }
  }

  private var baseQuery: [String: Any] {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
    ]
    if let accessGroup {
      query[kSecAttrAccessGroup as String] = accessGroup
    }
    return query
  }
}
