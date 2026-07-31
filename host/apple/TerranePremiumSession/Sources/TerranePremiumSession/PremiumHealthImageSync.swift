import CryptoKit
import Foundation
import Security

public let premiumHealthImageContract = "terrane.encrypted-image.v1"
public let premiumHealthAnalysisResultContract = "terrane.health-nutrition-result.v1"

public protocol PremiumHealthImageKeyStore: Sendable {
  func loadOrCreateKey() async throws -> Data
  func loadKey(id: String) async throws -> Data?
  func saveKey(_ key: Data, id: String) async throws
}

/// Process-local image-key storage for tests and unsigned development hosts.
/// It intentionally does not persist across process launches.
public actor PremiumVolatileHealthImageKeyStore: PremiumHealthImageKeyStore {
  private var currentKey: Data?
  private var keys: [String: Data]

  public init(key: Data? = nil) throws {
    if let key, key.count != 32 {
      throw PremiumHealthImageSyncError.invalidKey
    }
    currentKey = key
    keys = key.map { [premiumHealthImageKeyID($0): $0] } ?? [:]
  }

  public func loadOrCreateKey() async throws -> Data {
    if let currentKey {
      return currentKey
    }
    let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
    currentKey = key
    keys[premiumHealthImageKeyID(key)] = key
    return key
  }

  public func loadKey(id: String) async throws -> Data? {
    keys[id]
  }

  public func saveKey(_ key: Data, id: String) async throws {
    guard key.count == 32, premiumHealthImageKeyID(key) == id else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    keys[id] = key
  }
}

public struct PremiumStaticHealthImageKeyStore: PremiumHealthImageKeyStore {
  private let key: Data

  public init(key: Data) throws {
    guard key.count == 32 else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    self.key = key
  }

  public func loadOrCreateKey() async throws -> Data {
    key
  }

  public func loadKey(id: String) async throws -> Data? {
    id == premiumHealthImageKeyID(key) ? key : nil
  }

  public func saveKey(_ key: Data, id: String) async throws {
    guard key == self.key, id == premiumHealthImageKeyID(key) else {
      throw PremiumHealthImageSyncError.invalidKey
    }
  }
}

public struct PremiumKeychainHealthImageKeyStore: PremiumHealthImageKeyStore, Sendable {
  private let service: String
  private let account: String

  public init(
    service: String = "com.terrane.health.sync",
    account: String = "aes-256-gcm-key"
  ) {
    self.service = service
    self.account = account
  }

  public func loadOrCreateKey() async throws -> Data {
    if let key = try read(account: account) {
      return key
    }
    let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
    try save(key, account: account)
    try save(key, account: keyAccount(premiumHealthImageKeyID(key)))
    return key
  }

  public func loadKey(id: String) async throws -> Data? {
    if let key = try read(account: keyAccount(id)) {
      return key
    }
    guard let current = try read(account: account),
      premiumHealthImageKeyID(current) == id
    else {
      return nil
    }
    return current
  }

  public func saveKey(_ key: Data, id: String) async throws {
    guard key.count == 32, premiumHealthImageKeyID(key) == id else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    try save(key, account: keyAccount(id))
  }

  private func read(account: String) throws -> Data? {
    var query = baseQuery(account: account)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let readStatus = SecItemCopyMatching(query as CFDictionary, &item)
    if readStatus == errSecSuccess {
      guard let key = item as? Data, key.count == 32 else {
        throw PremiumHealthImageSyncError.invalidKey
      }
      return key
    }
    if readStatus == errSecItemNotFound {
      return nil
    }
    guard readStatus == errSecSuccess else {
      throw PremiumHealthImageSyncError.keychain(readStatus)
    }
    return nil
  }

  private func save(_ key: Data, account: String) throws {
    var attributes = baseQuery(account: account)
    attributes[kSecValueData as String] = key
    attributes[kSecAttrAccessible as String] =
      kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    let addStatus = SecItemAdd(attributes as CFDictionary, nil)
    if addStatus == errSecDuplicateItem {
      let update: [String: Any] = [
        kSecValueData as String: key,
        kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
      ]
      let updateStatus = SecItemUpdate(
        baseQuery(account: account) as CFDictionary,
        update as CFDictionary
      )
      guard updateStatus == errSecSuccess else {
        throw PremiumHealthImageSyncError.keychain(updateStatus)
      }
      return
    }
    guard addStatus == errSecSuccess else {
      throw PremiumHealthImageSyncError.keychain(addStatus)
    }
  }

  private func baseQuery(account: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
    ]
  }

  private func keyAccount(_ id: String) -> String {
    "key-\(id)"
  }
}

public protocol PremiumHealthDeviceKeyStore: Sendable {
  func loadOrCreatePrivateKey() async throws -> Data
}

public struct PremiumStaticHealthDeviceKeyStore: PremiumHealthDeviceKeyStore {
  private let privateKey: Data

  public init(privateKey: Data) throws {
    guard privateKey.count == 32 else {
      throw PremiumHealthImageSyncError.invalidDeviceKey
    }
    self.privateKey = privateKey
  }

  public func loadOrCreatePrivateKey() async throws -> Data {
    privateKey
  }
}

public struct PremiumKeychainHealthDeviceKeyStore: PremiumHealthDeviceKeyStore, Sendable {
  private let service: String
  private let account: String

  public init(
    service: String = "com.terrane.health.sync",
    account: String = "curve25519-device-key"
  ) {
    self.service = service
    self.account = account
  }

  public func loadOrCreatePrivateKey() async throws -> Data {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
    ]
    var read = query
    read[kSecReturnData as String] = true
    read[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let readStatus = SecItemCopyMatching(read as CFDictionary, &item)
    if readStatus == errSecSuccess {
      guard let key = item as? Data, key.count == 32 else {
        throw PremiumHealthImageSyncError.invalidDeviceKey
      }
      return key
    }
    guard readStatus == errSecItemNotFound else {
      throw PremiumHealthImageSyncError.keychain(readStatus)
    }
    let key = Curve25519.KeyAgreement.PrivateKey().rawRepresentation
    var attributes = query
    attributes[kSecValueData as String] = key
    attributes[kSecAttrAccessible as String] =
      kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    let status = SecItemAdd(attributes as CFDictionary, nil)
    if status == errSecDuplicateItem {
      return try await loadOrCreatePrivateKey()
    }
    guard status == errSecSuccess else {
      throw PremiumHealthImageSyncError.keychain(status)
    }
    return key
  }
}

public enum PremiumHealthImageSyncError: Error, Equatable, Sendable {
  case invalidKey
  case invalidDeviceKey
  case invalidImage
  case invalidResponse
  case missingOrganization
  case missingDevice
  case keychain(OSStatus)
}

extension PremiumHealthImageSyncError: LocalizedError {
  public var errorDescription: String? {
    switch self {
    case .invalidKey:
      return "The Health sync key must contain exactly 32 bytes."
    case .invalidDeviceKey:
      return "The Health device key is invalid."
    case .invalidImage:
      return "The Health image is empty or too large."
    case .invalidResponse:
      return "The Premium Health image response is invalid."
    case .missingOrganization:
      return "The Premium account is not attached to an organization."
    case .missingDevice:
      return "This Premium session is not attached to an active device."
    case .keychain(let status):
      return "Health sync could not access Keychain (\(status))."
    }
  }
}

public struct PremiumHealthImageMetadata: Codable, Equatable, Sendable {
  public let mime: String
  public let width: Int?
  public let height: Int?
  public let capturedAt: Date
  public let source: String
  public let note: String?

  public init(
    mime: String = "image/jpeg",
    width: Int? = nil,
    height: Int? = nil,
    capturedAt: Date = Date(),
    source: String = "ios-health",
    note: String? = nil
  ) {
    self.mime = mime
    self.width = width
    self.height = height
    self.capturedAt = capturedAt
    self.source = source
    self.note = note
  }
}

public struct PremiumHealthImageAttachment: Codable, Equatable, Sendable {
  public let contract: String
  public let id: String
  public let orgId: String
  public let deviceId: String
  public let appId: String
  public let clientId: String
  public let imageSha256: String
  public let metadataSha256: String
  public let status: String
  public let createdAt: Date
}

public struct PremiumDecryptedHealthImage: Equatable, Sendable {
  public let attachment: PremiumHealthImageAttachment
  public let image: Data
  public let metadata: PremiumHealthImageMetadata
  public let keyID: String
}

public struct PremiumHealthAnalyzerStatus: Codable, Equatable, Sendable {
  public let ready: Bool
  public let lastSeenAt: Date?
  public let expiresAt: Date?
}

public struct PremiumHealthConnectionDevice: Codable, Equatable, Sendable, Identifiable {
  public var id: String { deviceId }
  public let deviceId: String
  public let name: String
  public let platform: String
  public let clientVersion: String?
  public let keyRegistered: Bool
  public let keyFingerprint: String?
  public let analyzer: PremiumHealthAnalyzerStatus?
}

public struct PremiumHealthConnectionPairing: Codable, Equatable, Sendable {
  public let senderDeviceId: String
  public let recipientDeviceId: String
  public let status: String
  public let senderKeyFingerprint: String
  public let recipientKeyFingerprint: String
  public let approvedAt: Date
}

public struct PremiumHealthConnections: Codable, Equatable, Sendable {
  public let devices: [PremiumHealthConnectionDevice]
  public let pairings: [PremiumHealthConnectionPairing]
}

public struct PremiumHealthAnalysisJob: Codable, Equatable, Sendable, Identifiable {
  public let contract: String
  public let id: String
  public let kind: String
  public let sourceImageId: String
  public let status: String
  public let revision: Int
  public let attempt: Int
  public let nextAttemptAt: Date?
  public let leaseExpiresAt: Date?
  public let resultAvailable: Bool
  public let failureCode: String?
  public let createdAt: Date
  public let updatedAt: Date
  public let completedAt: Date?
  public let acknowledgedAt: Date?
}

public struct PremiumHealthAnalysisLease: Codable, Equatable, Sendable {
  public let claimId: String
  public let claimToken: String
  public let leaseEpoch: Int
  public let leaseExpiresAt: Date
}

public struct PremiumHealthAnalysisClaim: Equatable, Sendable {
  public let job: PremiumHealthAnalysisJob?
  public let lease: PremiumHealthAnalysisLease?
  public let cursor: Int
}

public struct PremiumHealthAnalysisPage: Equatable, Sendable {
  public let jobs: [PremiumHealthAnalysisJob]
  public let cursor: Int
}

public struct PremiumDecryptedHealthAnalysis: Equatable, Sendable {
  public let job: PremiumHealthAnalysisJob
  public let image: PremiumDecryptedHealthImage
  public let result: Data
}

public actor PremiumHealthImageSyncClient {
  private struct Organization: Decodable, Sendable {
    let id: String
  }

  private struct Device: Decodable, Sendable {
    let id: String
    let platform: String
    let status: String
  }

  private struct Envelope: Codable, Equatable, Sendable {
    let algorithm: String
    let keyId: String
    let iv: String
    let ciphertext: String
    let authTag: String
  }

  private struct UploadRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let appId: String
    let clientId: String
    let imageEnvelope: Envelope
    let metadataEnvelope: Envelope
  }

  private struct UploadResponse: Decodable, Sendable {
    let attachment: PremiumHealthImageAttachment
    let idempotent: Bool
  }

  private struct ListResponse: Decodable, Sendable {
    let attachments: [PremiumHealthImageAttachment]
  }

  private struct DownloadResponse: Decodable, Sendable {
    let attachment: PremiumHealthImageAttachment
    let imageEnvelope: Envelope
    let metadataEnvelope: Envelope
  }

  private struct RegisterDeviceKeyRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let publicKey: String
  }

  private struct RegisteredDeviceKey: Decodable, Sendable {
    let deviceId: String
    let publicKey: String
    let fingerprint: String
  }

  private struct HealthDeviceKey: Decodable, Sendable {
    let deviceId: String
    let platform: String
    let publicKey: String
    let fingerprint: String
  }

  private struct DeviceKeysResponse: Decodable, Sendable {
    let devices: [HealthDeviceKey]
  }

  private struct PairingRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let senderDeviceId: String
    let senderKeyFingerprint: String
    let approved: Bool
  }

  private struct Pairing: Decodable, Sendable {
    let orgId: String
    let senderDeviceId: String
    let recipientDeviceId: String
    let senderKeyFingerprint: String
    let recipientKeyFingerprint: String
    let status: String
  }

  private struct WrappedKeyEnvelope: Codable, Sendable {
    let contract: String
    let algorithm: String
    let ephemeralPublicKey: String
    let recipientKeyFingerprint: String
    let salt: String
    let iv: String
    let ciphertext: String
    let authTag: String
    let contextSha256: String
  }

  private struct GrantKeyRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let imageId: String
    let recipientDeviceId: String
    let wrappedKeyEnvelope: WrappedKeyEnvelope
  }

  private struct KeyGrant: Decodable, Sendable {
    let orgId: String
    let imageId: String
    let senderDeviceId: String
    let recipientDeviceId: String
    let senderPublicKey: String
    let senderKeyFingerprint: String
    let recipientKeyFingerprint: String
    let keyId: String
    let wrappedKeyEnvelope: WrappedKeyEnvelope?
  }

  private struct AnalyzerHeartbeatRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let ready: Bool
  }

  private struct PairingRevokeResponse: Decodable, Sendable {
    let revoked: Bool
    let senderDeviceId: String
    let recipientDeviceId: String
    let removedGrantCount: Int
    let requeuedJobCount: Int
  }

  private struct CreateAnalysisJobRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let imageId: String
    let clientRequestId: String
    let kind: String
  }

  private struct CreateAnalysisJobResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let idempotent: Bool
  }

  private struct ClaimAnalysisJobRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let claimId: String
    let claimToken: String
    let waitSeconds: Int
  }

  private struct ClaimAnalysisJobResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob?
    let lease: PremiumHealthAnalysisLease?
    let cursor: Int
  }

  private struct AnalysisJobsResponse: Decodable, Sendable {
    let jobs: [PremiumHealthAnalysisJob]
    let cursor: Int
  }

  private struct AnalysisLeaseRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let claimId: String
    let claimToken: String
    let leaseEpoch: Int
  }

  private struct AnalysisLeaseResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let lease: RefreshedAnalysisLease
  }

  private struct RefreshedAnalysisLease: Decodable, Sendable {
    let leaseEpoch: Int
    let leaseExpiresAt: Date
  }

  private struct CompleteAnalysisJobRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let claimId: String
    let claimToken: String
    let leaseEpoch: Int
    let completionId: String
    let resultEnvelope: Envelope
  }

  private struct CompleteAnalysisJobResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let idempotent: Bool
  }

  private struct FailAnalysisJobRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let claimId: String
    let claimToken: String
    let leaseEpoch: Int
    let failureId: String
    let code: String
    let retryable: Bool
  }

  private struct FailAnalysisJobResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let idempotent: Bool
  }

  private struct AnalysisRequesterRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let clientRequestId: String
  }

  private struct AnalysisRequesterResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let idempotent: Bool
  }

  private struct AnalysisResultResponse: Decodable, Sendable {
    let job: PremiumHealthAnalysisJob
    let resultEnvelope: Envelope
  }

  private let session: PremiumSessionClient
  private let keyStore: any PremiumHealthImageKeyStore
  private let deviceKeyStore: any PremiumHealthDeviceKeyStore
  private let platform: PremiumPlatform
  private let encoder: JSONEncoder
  private let decoder: JSONDecoder
  private var cachedContext: (orgID: String, deviceID: String)?

  public init(
    session: PremiumSessionClient,
    keyStore: any PremiumHealthImageKeyStore,
    deviceKeyStore: any PremiumHealthDeviceKeyStore =
      PremiumKeychainHealthDeviceKeyStore(),
    platform: PremiumPlatform
  ) {
    self.session = session
    self.keyStore = keyStore
    self.deviceKeyStore = deviceKeyStore
    self.platform = platform
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    self.encoder = encoder
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .custom(PremiumDateCoding.decode)
    self.decoder = decoder
  }

  public func upload(
    image: Data,
    metadata: PremiumHealthImageMetadata,
    clientID: String = UUID().uuidString.lowercased()
  ) async throws -> PremiumHealthImageAttachment {
    guard !image.isEmpty, image.count <= 12 * 1024 * 1024 else {
      throw PremiumHealthImageSyncError.invalidImage
    }
    let context = try await resolveContext()
    let keyData = try await keyStore.loadOrCreateKey()
    let keyID = premiumHealthImageKeyID(keyData)
    let imageEnvelope = try Self.seal(
      image,
      keyData: keyData,
      keyID: keyID,
      authenticatedData: Self.authenticatedData(
        orgID: context.orgID, clientID: clientID, part: "image")
    )
    let metadataEnvelope = try Self.seal(
      encoder.encode(metadata),
      keyData: keyData,
      keyID: keyID,
      authenticatedData: Self.authenticatedData(
        orgID: context.orgID, clientID: clientID, part: "metadata")
    )
    let response: UploadResponse = try await session.send(
      path: "sync/images",
      method: .post,
      body: UploadRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        appId: "health",
        clientId: clientID,
        imageEnvelope: imageEnvelope,
        metadataEnvelope: metadataEnvelope
      )
    )
    guard response.attachment.contract == premiumHealthImageContract else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    try await grantKey(
      keyData,
      keyID: keyID,
      attachment: response.attachment,
      context: context
    )
    return response.attachment
  }

  public func list() async throws -> [PremiumHealthImageAttachment] {
    let context = try await resolveContext()
    _ = try await registerDeviceKey(context: context)
    let query = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
      URLQueryItem(name: "appId", value: "health"),
    ]
    var components = URLComponents()
    components.path = "sync/images"
    components.queryItems = query
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let response: ListResponse = try await session.send(path: path)
    return response.attachments
  }

  public func approveSenders() async throws {
    let context = try await resolveContext()
    let registered = try await registerDeviceKey(context: context)
    let peers = try await listDeviceKeys(context: context)
    for peer in peers.devices where peer.deviceId != context.deviceID {
      let pairing: Pairing = try await session.send(
        path: "sync/device-key-pairings",
        method: .post,
        body: PairingRequest(
          orgId: context.orgID,
          deviceId: context.deviceID,
          senderDeviceId: peer.deviceId,
          senderKeyFingerprint: peer.fingerprint,
          approved: true
        )
      )
      guard pairing.orgId == context.orgID,
        pairing.senderDeviceId == peer.deviceId,
        pairing.recipientDeviceId == context.deviceID,
        pairing.senderKeyFingerprint == peer.fingerprint,
        pairing.recipientKeyFingerprint == registered.fingerprint,
        pairing.status == "approved"
      else {
        throw PremiumHealthImageSyncError.invalidResponse
      }
    }
  }

  public func currentDeviceID() async throws -> String {
    try await resolveContext().deviceID
  }

  public func connections() async throws -> PremiumHealthConnections {
    let context = try await resolveContext()
    _ = try await registerDeviceKey(context: context)
    var components = URLComponents()
    components.path = "sync/health-connections"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return try await session.send(path: path)
  }

  public func heartbeatAnalyzer(ready: Bool) async throws -> PremiumHealthAnalyzerStatus {
    let context = try await resolveContext()
    return try await session.send(
      path: "sync/health-analyzers/heartbeat",
      method: .post,
      body: AnalyzerHeartbeatRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        ready: ready
      )
    )
  }

  public func approveSender(deviceID senderDeviceID: String) async throws {
    let context = try await resolveContext()
    let registered = try await registerDeviceKey(context: context)
    let peers = try await listDeviceKeys(context: context)
    guard let sender = peers.devices.first(where: { $0.deviceId == senderDeviceID }),
      sender.deviceId != context.deviceID
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let pairing: Pairing = try await session.send(
      path: "sync/device-key-pairings",
      method: .post,
      body: PairingRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        senderDeviceId: sender.deviceId,
        senderKeyFingerprint: sender.fingerprint,
        approved: true
      )
    )
    guard pairing.orgId == context.orgID,
      pairing.senderDeviceId == sender.deviceId,
      pairing.recipientDeviceId == context.deviceID,
      pairing.senderKeyFingerprint == sender.fingerprint,
      pairing.recipientKeyFingerprint == registered.fingerprint,
      pairing.status == "approved"
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
  }

  @discardableResult
  public func revokeSender(deviceID senderDeviceID: String) async throws -> Bool {
    let context = try await resolveContext()
    var components = URLComponents()
    components.path = "sync/device-key-pairings/\(senderDeviceID)"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let response: PairingRevokeResponse = try await session.send(
      path: path,
      method: .delete
    )
    guard response.senderDeviceId == senderDeviceID,
      response.recipientDeviceId == context.deviceID
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return response.revoked
  }

  public func createAnalysisJob(
    imageID: String,
    clientRequestID: String = UUID().uuidString.lowercased()
  ) async throws -> PremiumHealthAnalysisJob {
    let context = try await resolveContext()
    let response: CreateAnalysisJobResponse = try await session.send(
      path: "sync/health-analysis/jobs",
      method: .post,
      body: CreateAnalysisJobRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        imageId: imageID,
        clientRequestId: clientRequestID,
        kind: "food_nutrition"
      )
    )
    guard response.job.contract == "terrane.health-analysis-job.v1",
      response.job.sourceImageId == imageID,
      response.job.kind == "food_nutrition"
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return response.job
  }

  public nonisolated static func makeClaimToken() -> String {
    randomData(count: 32).base64URLEncodedString()
  }

  public func claimAnalysisJob(
    claimID: String = UUID().uuidString.lowercased(),
    claimToken: String = PremiumHealthImageSyncClient.makeClaimToken(),
    waitSeconds: Int = 0
  ) async throws -> PremiumHealthAnalysisClaim {
    let context = try await resolveContext()
    let response: ClaimAnalysisJobResponse = try await session.send(
      path: "sync/health-analysis/jobs/claim",
      method: .post,
      body: ClaimAnalysisJobRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        claimId: claimID,
        claimToken: claimToken,
        waitSeconds: min(max(waitSeconds, 0), 25)
      )
    )
    guard (response.job == nil) == (response.lease == nil) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return PremiumHealthAnalysisClaim(
      job: response.job,
      lease: response.lease,
      cursor: response.cursor
    )
  }

  public func heartbeatAnalysisJob(
    _ job: PremiumHealthAnalysisJob,
    lease: PremiumHealthAnalysisLease
  ) async throws -> PremiumHealthAnalysisLease {
    let context = try await resolveContext()
    let response: AnalysisLeaseResponse = try await session.send(
      path: "sync/health-analysis/jobs/\(job.id)/heartbeat",
      method: .post,
      body: AnalysisLeaseRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        claimId: lease.claimId,
        claimToken: lease.claimToken,
        leaseEpoch: lease.leaseEpoch
      )
    )
    guard response.job.id == job.id,
      response.lease.leaseEpoch == lease.leaseEpoch
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return PremiumHealthAnalysisLease(
      claimId: lease.claimId,
      claimToken: lease.claimToken,
      leaseEpoch: response.lease.leaseEpoch,
      leaseExpiresAt: response.lease.leaseExpiresAt
    )
  }

  public func completeAnalysisJob(
    _ job: PremiumHealthAnalysisJob,
    lease: PremiumHealthAnalysisLease,
    image: PremiumDecryptedHealthImage,
    result: Data,
    completionID: String = UUID().uuidString.lowercased()
  ) async throws -> PremiumHealthAnalysisJob {
    guard job.sourceImageId == image.attachment.id,
      let imageKey = try await keyStore.loadKey(id: image.keyID)
    else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    let context = try await resolveContext()
    let resultKey = Self.analysisResultKey(
      imageKey: imageKey,
      orgID: context.orgID,
      imageID: job.sourceImageId,
      jobID: job.id
    )
    let envelope = try Self.seal(
      result,
      keyData: resultKey,
      keyID: premiumHealthImageKeyID(resultKey),
      authenticatedData: Self.analysisResultAuthenticatedData(
        orgID: context.orgID,
        imageID: job.sourceImageId,
        jobID: job.id
      )
    )
    let response: CompleteAnalysisJobResponse = try await session.send(
      path: "sync/health-analysis/jobs/\(job.id)/complete",
      method: .post,
      body: CompleteAnalysisJobRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        claimId: lease.claimId,
        claimToken: lease.claimToken,
        leaseEpoch: lease.leaseEpoch,
        completionId: completionID,
        resultEnvelope: envelope
      )
    )
    guard response.job.id == job.id, response.job.resultAvailable else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return response.job
  }

  public func failAnalysisJob(
    _ job: PremiumHealthAnalysisJob,
    lease: PremiumHealthAnalysisLease,
    code: String,
    retryable: Bool,
    failureID: String = UUID().uuidString.lowercased()
  ) async throws -> PremiumHealthAnalysisJob {
    let allowedCodes = [
      "analysis_failed", "decrypt_failed", "invalid_image", "model_unavailable", "timeout",
    ]
    guard allowedCodes.contains(code) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let context = try await resolveContext()
    let response: FailAnalysisJobResponse = try await session.send(
      path: "sync/health-analysis/jobs/\(job.id)/fail",
      method: .post,
      body: FailAnalysisJobRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        claimId: lease.claimId,
        claimToken: lease.claimToken,
        leaseEpoch: lease.leaseEpoch,
        failureId: failureID,
        code: code,
        retryable: retryable
      )
    )
    guard response.job.id == job.id else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return response.job
  }

  public func analysisJobs(
    afterRevision: Int = 0,
    waitSeconds: Int = 0
  ) async throws -> PremiumHealthAnalysisPage {
    let context = try await resolveContext()
    var components = URLComponents()
    components.path = "sync/health-analysis/jobs"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
      URLQueryItem(name: "afterRevision", value: String(max(afterRevision, 0))),
      URLQueryItem(name: "waitSeconds", value: String(min(max(waitSeconds, 0), 25))),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let response: AnalysisJobsResponse = try await session.send(path: path)
    return PremiumHealthAnalysisPage(jobs: response.jobs, cursor: response.cursor)
  }

  public func downloadAnalysisResult(
    _ job: PremiumHealthAnalysisJob
  ) async throws -> PremiumDecryptedHealthAnalysis {
    guard job.resultAvailable else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let context = try await resolveContext()
    guard let attachment = try await list().first(where: { $0.id == job.sourceImageId }) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let image = try await download(attachment)
    guard let imageKey = try await keyStore.loadKey(id: image.keyID) else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    var components = URLComponents()
    components.path = "sync/health-analysis/jobs/\(job.id)/result"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let response: AnalysisResultResponse = try await session.send(path: path)
    guard response.job.id == job.id,
      response.job.sourceImageId == image.attachment.id
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let resultKey = Self.analysisResultKey(
      imageKey: imageKey,
      orgID: context.orgID,
      imageID: job.sourceImageId,
      jobID: job.id
    )
    let result = try Self.open(
      response.resultEnvelope,
      keyData: resultKey,
      authenticatedData: Self.analysisResultAuthenticatedData(
        orgID: context.orgID,
        imageID: job.sourceImageId,
        jobID: job.id
      )
    )
    return PremiumDecryptedHealthAnalysis(job: response.job, image: image, result: result)
  }

  public func cancelAnalysisJob(
    _ job: PremiumHealthAnalysisJob,
    clientRequestID: String
  ) async throws -> PremiumHealthAnalysisJob {
    try await updateRequesterJob(
      job,
      action: "cancel",
      clientRequestID: clientRequestID
    )
  }

  public func acknowledgeAnalysisJob(
    _ job: PremiumHealthAnalysisJob,
    clientRequestID: String
  ) async throws -> PremiumHealthAnalysisJob {
    try await updateRequesterJob(
      job,
      action: "ack",
      clientRequestID: clientRequestID
    )
  }

  public func download(_ attachment: PremiumHealthImageAttachment) async throws
    -> PremiumDecryptedHealthImage
  {
    let context = try await resolveContext()
    var components = URLComponents()
    components.path = "sync/images/\(attachment.id)"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let response: DownloadResponse = try await session.send(path: path)
    guard response.attachment == attachment else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let keyData: Data
    if let stored = try await keyStore.loadKey(id: response.imageEnvelope.keyId) {
      keyData = stored
    } else {
      keyData = try await unwrapKey(
        id: response.imageEnvelope.keyId,
        attachment: attachment,
        context: context
      )
      try await keyStore.saveKey(keyData, id: response.imageEnvelope.keyId)
    }
    let image = try Self.open(
      response.imageEnvelope,
      keyData: keyData,
      authenticatedData: Self.authenticatedData(
        orgID: context.orgID, clientID: attachment.clientId, part: "image")
    )
    let metadataData = try Self.open(
      response.metadataEnvelope,
      keyData: keyData,
      authenticatedData: Self.authenticatedData(
        orgID: context.orgID, clientID: attachment.clientId, part: "metadata")
    )
    let metadata = try decoder.decode(PremiumHealthImageMetadata.self, from: metadataData)
    return PremiumDecryptedHealthImage(
      attachment: attachment,
      image: image,
      metadata: metadata,
      keyID: response.imageEnvelope.keyId
    )
  }

  private func registerDeviceKey(
    context: (orgID: String, deviceID: String)
  ) async throws -> (
    privateKey: Curve25519.KeyAgreement.PrivateKey, fingerprint: String
  ) {
    let rawPrivateKey = try await deviceKeyStore.loadOrCreatePrivateKey()
    let privateKey = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: rawPrivateKey)
    let request = RegisterDeviceKeyRequest(
      orgId: context.orgID,
      deviceId: context.deviceID,
      publicKey: privateKey.publicKey.rawRepresentation.base64URLEncodedString()
    )
    let registered: RegisteredDeviceKey = try await session.send(
      path: "sync/device-keys",
      method: .post,
      body: request
    )
    guard registered.deviceId == context.deviceID,
      registered.publicKey == request.publicKey,
      registered.fingerprint == Self.keyFingerprint(privateKey.publicKey.rawRepresentation)
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return (privateKey, registered.fingerprint)
  }

  private func listDeviceKeys(
    context: (orgID: String, deviceID: String)
  ) async throws -> DeviceKeysResponse {
    var components = URLComponents()
    components.path = "sync/device-keys"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return try await session.send(path: path)
  }

  private func grantKey(
    _ keyData: Data,
    keyID: String,
    attachment: PremiumHealthImageAttachment,
    context: (orgID: String, deviceID: String)
  ) async throws {
    _ = try await registerDeviceKey(context: context)
    let peers = try await listDeviceKeys(context: context)
    for peer in peers.devices where peer.deviceId != context.deviceID {
      guard
        peer.fingerprint
          == Self.keyFingerprint(try Self.decodeBase64URL(peer.publicKey))
      else {
        throw PremiumHealthImageSyncError.invalidResponse
      }
      let wrappedKeyEnvelope = try Self.wrapKey(
        keyData,
        recipientPublicKey: try Self.decodeBase64URL(peer.publicKey),
        recipientKeyFingerprint: peer.fingerprint,
        orgID: context.orgID,
        imageID: attachment.id,
        senderDeviceID: context.deviceID,
        recipientDeviceID: peer.deviceId,
        senderKeyFingerprint: Self.keyFingerprint(
          try Curve25519.KeyAgreement.PrivateKey(
            rawRepresentation: try await deviceKeyStore.loadOrCreatePrivateKey()
          ).publicKey.rawRepresentation
        ),
        keyID: keyID
      )
      let _: KeyGrant = try await session.send(
        path: "sync/image-key-grants",
        method: .post,
        body: GrantKeyRequest(
          orgId: context.orgID,
          deviceId: context.deviceID,
          imageId: attachment.id,
          recipientDeviceId: peer.deviceId,
          wrappedKeyEnvelope: wrappedKeyEnvelope
        )
      )
    }
  }

  private func unwrapKey(
    id keyID: String,
    attachment: PremiumHealthImageAttachment,
    context: (orgID: String, deviceID: String)
  ) async throws -> Data {
    let registered = try await registerDeviceKey(context: context)
    var components = URLComponents()
    components.path = "sync/image-key-grants/\(attachment.id)"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let grant: KeyGrant = try await session.send(path: path)
    guard grant.orgId == context.orgID,
      grant.imageId == attachment.id,
      grant.recipientDeviceId == context.deviceID,
      grant.keyId == keyID,
      grant.recipientKeyFingerprint == registered.fingerprint,
      let wrappedKeyEnvelope = grant.wrappedKeyEnvelope
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    guard
      grant.senderKeyFingerprint
        == Self.keyFingerprint(try Self.decodeBase64URL(grant.senderPublicKey))
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let key = try Self.unwrapKey(
      wrappedKeyEnvelope,
      privateKey: registered.privateKey,
      recipientKeyFingerprint: registered.fingerprint,
      orgID: context.orgID,
      imageID: attachment.id,
      senderDeviceID: grant.senderDeviceId,
      recipientDeviceID: context.deviceID,
      senderKeyFingerprint: grant.senderKeyFingerprint,
      keyID: keyID
    )
    guard key.count == 32, premiumHealthImageKeyID(key) == keyID else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    return key
  }

  private func updateRequesterJob(
    _ job: PremiumHealthAnalysisJob,
    action: String,
    clientRequestID: String
  ) async throws -> PremiumHealthAnalysisJob {
    let context = try await resolveContext()
    let response: AnalysisRequesterResponse = try await session.send(
      path: "sync/health-analysis/jobs/\(job.id)/\(action)",
      method: .post,
      body: AnalysisRequesterRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        clientRequestId: clientRequestID
      )
    )
    guard response.job.id == job.id else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return response.job
  }

  private static func wrapKey(
    _ keyData: Data,
    recipientPublicKey: Data,
    recipientKeyFingerprint: String,
    orgID: String,
    imageID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderKeyFingerprint: String,
    keyID: String
  ) throws -> WrappedKeyEnvelope {
    let ephemeral = Curve25519.KeyAgreement.PrivateKey()
    let recipient = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: recipientPublicKey)
    let shared = try ephemeral.sharedSecretFromKeyAgreement(with: recipient)
    let salt = Self.randomData(count: 32)
    let context = Self.wrappedKeyContext(
      orgID: orgID,
      imageID: imageID,
      keyID: keyID,
      senderDeviceID: senderDeviceID,
      recipientDeviceID: recipientDeviceID,
      senderKeyFingerprint: senderKeyFingerprint,
      recipientKeyFingerprint: recipientKeyFingerprint
    )
    let wrappingKey = shared.hkdfDerivedSymmetricKey(
      using: SHA256.self,
      salt: salt,
      sharedInfo: Data("terrane.health-wrapped-key.v1".utf8),
      outputByteCount: 32
    )
    let sealed = try AES.GCM.seal(
      keyData,
      using: wrappingKey,
      authenticating: context
    )
    return WrappedKeyEnvelope(
      contract: "terrane.health-wrapped-key.v1",
      algorithm: "x25519-hkdf-sha256-aes-256-gcm",
      ephemeralPublicKey: ephemeral.publicKey.rawRepresentation.base64URLEncodedString(),
      recipientKeyFingerprint: recipientKeyFingerprint,
      salt: salt.base64URLEncodedString(),
      iv: Data(sealed.nonce).base64URLEncodedString(),
      ciphertext: sealed.ciphertext.base64URLEncodedString(),
      authTag: sealed.tag.base64URLEncodedString(),
      contextSha256: Self.sha256Identifier(context)
    )
  }

  private static func unwrapKey(
    _ envelope: WrappedKeyEnvelope,
    privateKey: Curve25519.KeyAgreement.PrivateKey,
    recipientKeyFingerprint: String,
    orgID: String,
    imageID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderKeyFingerprint: String,
    keyID: String
  ) throws -> Data {
    guard envelope.contract == "terrane.health-wrapped-key.v1",
      envelope.algorithm == "x25519-hkdf-sha256-aes-256-gcm",
      envelope.recipientKeyFingerprint == recipientKeyFingerprint
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let context = Self.wrappedKeyContext(
      orgID: orgID,
      imageID: imageID,
      keyID: keyID,
      senderDeviceID: senderDeviceID,
      recipientDeviceID: recipientDeviceID,
      senderKeyFingerprint: senderKeyFingerprint,
      recipientKeyFingerprint: recipientKeyFingerprint
    )
    guard envelope.contextSha256 == Self.sha256Identifier(context) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let ephemeral = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: try Self.decodeBase64URL(envelope.ephemeralPublicKey))
    let salt = try Self.decodeBase64URL(envelope.salt)
    guard salt.count == 32 else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let shared = try privateKey.sharedSecretFromKeyAgreement(with: ephemeral)
    let wrappingKey = shared.hkdfDerivedSymmetricKey(
      using: SHA256.self,
      salt: salt,
      sharedInfo: Data("terrane.health-wrapped-key.v1".utf8),
      outputByteCount: 32
    )
    let box = try AES.GCM.SealedBox(
      nonce: AES.GCM.Nonce(data: try Self.decodeBase64URL(envelope.iv)),
      ciphertext: try Self.decodeBase64URL(envelope.ciphertext),
      tag: try Self.decodeBase64URL(envelope.authTag)
    )
    return try AES.GCM.open(box, using: wrappingKey, authenticating: context)
  }

  private static func wrappedKeyContext(
    orgID: String,
    imageID: String,
    keyID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderKeyFingerprint: String,
    recipientKeyFingerprint: String
  ) -> Data {
    Data(
      """
      {"contract":"terrane.health-wrapped-key.v1","orgId":"\(orgID)","imageId":"\(imageID)","keyId":"\(keyID)","senderDeviceId":"\(senderDeviceID)","recipientDeviceId":"\(recipientDeviceID)","senderKeyFingerprint":"\(senderKeyFingerprint)","recipientKeyFingerprint":"\(recipientKeyFingerprint)"}
      """.utf8
    )
  }

  private static func keyFingerprint(_ publicKey: Data) -> String {
    sha256Identifier(publicKey)
  }

  private static func sha256Identifier(_ data: Data) -> String {
    "sha256:\(SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined())"
  }

  private static func randomData(count: Int) -> Data {
    var bytes = [UInt8](repeating: 0, count: count)
    _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
    return Data(bytes)
  }

  private static func decodeBase64URL(_ value: String) throws -> Data {
    guard let data = Data(base64URLString: value) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    return data
  }

  private func resolveContext() async throws -> (orgID: String, deviceID: String) {
    if let cachedContext {
      return cachedContext
    }
    let organizations: [Organization] = try await session.send(path: "users/me/orgs")
    guard let orgID = organizations.first?.id else {
      throw PremiumHealthImageSyncError.missingOrganization
    }
    let devices: [Device] = try await session.send(path: "users/me/devices")
    let sessionDeviceID = await session.currentDeviceID
    let platformName = platform.rawValue
    guard
      let deviceID =
        sessionDeviceID
        ?? devices.first(where: { $0.platform == platformName && $0.status == "active" })?.id
    else {
      throw PremiumHealthImageSyncError.missingDevice
    }
    let context = (orgID: orgID, deviceID: deviceID)
    cachedContext = context
    return context
  }

  private static func seal(
    _ plaintext: Data,
    keyData: Data,
    keyID: String,
    authenticatedData: Data
  ) throws -> Envelope {
    guard keyData.count == 32 else {
      throw PremiumHealthImageSyncError.invalidKey
    }
    let sealed = try AES.GCM.seal(
      plaintext,
      using: SymmetricKey(data: keyData),
      authenticating: authenticatedData
    )
    return Envelope(
      algorithm: "aes-256-gcm",
      keyId: keyID,
      iv: Data(sealed.nonce).base64URLEncodedString(),
      ciphertext: sealed.ciphertext.base64URLEncodedString(),
      authTag: sealed.tag.base64URLEncodedString()
    )
  }

  private static func open(
    _ envelope: Envelope,
    keyData: Data,
    authenticatedData: Data
  ) throws -> Data {
    guard envelope.algorithm == "aes-256-gcm",
      envelope.keyId == premiumHealthImageKeyID(keyData),
      let nonceData = Data(base64URLString: envelope.iv),
      let ciphertext = Data(base64URLString: envelope.ciphertext),
      let tag = Data(base64URLString: envelope.authTag)
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    let box = try AES.GCM.SealedBox(
      nonce: AES.GCM.Nonce(data: nonceData),
      ciphertext: ciphertext,
      tag: tag
    )
    return try AES.GCM.open(
      box,
      using: SymmetricKey(data: keyData),
      authenticating: authenticatedData
    )
  }

  private static func authenticatedData(orgID: String, clientID: String, part: String) -> Data {
    Data("\(premiumHealthImageContract):\(orgID):health:\(clientID):\(part)".utf8)
  }

  private static func analysisResultKey(
    imageKey: Data,
    orgID: String,
    imageID: String,
    jobID: String
  ) -> Data {
    let key = HKDF<SHA256>.deriveKey(
      inputKeyMaterial: SymmetricKey(data: imageKey),
      salt: Data(
        "\(premiumHealthAnalysisResultContract):\(orgID):\(imageID):\(jobID):salt".utf8
      ),
      info: Data(premiumHealthAnalysisResultContract.utf8),
      outputByteCount: 32
    )
    return key.withUnsafeBytes { Data($0) }
  }

  private static func analysisResultAuthenticatedData(
    orgID: String,
    imageID: String,
    jobID: String
  ) -> Data {
    Data(
      "\(premiumHealthAnalysisResultContract):\(orgID):\(imageID):\(jobID):result".utf8
    )
  }
}

private func premiumHealthImageKeyID(_ key: Data) -> String {
  "health-\(SHA256.hash(data: key).prefix(12).map { String(format: "%02x", $0) }.joined())"
}

extension Data {
  fileprivate init?(base64URLString: String) {
    var value = base64URLString.replacingOccurrences(of: "-", with: "+")
      .replacingOccurrences(of: "_", with: "/")
    value.append(String(repeating: "=", count: (4 - value.count % 4) % 4))
    self.init(base64Encoded: value)
  }

  fileprivate func base64URLEncodedString() -> String {
    base64EncodedString()
      .replacingOccurrences(of: "+", with: "-")
      .replacingOccurrences(of: "/", with: "_")
      .replacingOccurrences(of: "=", with: "")
  }
}
