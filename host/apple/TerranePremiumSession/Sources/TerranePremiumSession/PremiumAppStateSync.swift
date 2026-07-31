import CryptoKit
import Foundation
import Security

public let premiumAppStateSyncContract = "terrane.encrypted-crdt-update.v1"

public func premiumAppSyncUpdateID(_ update: Data) -> String {
  SHA256.hash(data: update).map { String(format: "%02x", $0) }.joined()
}

public protocol PremiumAppSyncKeyStore: Sendable {
  func loadOrCreateKey() async throws -> Data
  func loadKey(id: String) async throws -> Data?
  func saveKey(_ key: Data, id: String) async throws
}

public actor PremiumVolatileAppSyncKeyStore: PremiumAppSyncKeyStore {
  private var current: Data?
  private var keys: [String: Data] = [:]

  public init(key: Data? = nil) throws {
    if let key, key.count != 32 {
      throw PremiumAppStateSyncError.invalidKey
    }
    current = key
    if let key {
      keys[premiumAppSyncKeyID(key)] = key
    }
  }

  public func loadOrCreateKey() async throws -> Data {
    if let current {
      return current
    }
    let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
    current = key
    keys[premiumAppSyncKeyID(key)] = key
    return key
  }

  public func loadKey(id: String) async throws -> Data? {
    keys[id]
  }

  public func saveKey(_ key: Data, id: String) async throws {
    guard key.count == 32, premiumAppSyncKeyID(key) == id else {
      throw PremiumAppStateSyncError.invalidKey
    }
    keys[id] = key
  }
}

public struct PremiumKeychainAppSyncKeyStore: PremiumAppSyncKeyStore, Sendable {
  private let service: String
  private let account: String

  public init(
    service: String = "com.terrane.app-state.sync",
    account: String = "aes-256-gcm-key"
  ) {
    self.service = service
    self.account = account
  }

  public func loadOrCreateKey() async throws -> Data {
    if let key = try read(account) {
      return key
    }
    let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
    try write(key, account)
    try write(key, keyAccount(premiumAppSyncKeyID(key)))
    return key
  }

  public func loadKey(id: String) async throws -> Data? {
    if let key = try read(keyAccount(id)) {
      return key
    }
    guard let current = try read(account), premiumAppSyncKeyID(current) == id else {
      return nil
    }
    return current
  }

  public func saveKey(_ key: Data, id: String) async throws {
    guard key.count == 32, premiumAppSyncKeyID(key) == id else {
      throw PremiumAppStateSyncError.invalidKey
    }
    try write(key, keyAccount(id))
  }

  private func read(_ account: String) throws -> Data? {
    var query = baseQuery(account)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &item)
    if status == errSecItemNotFound {
      return nil
    }
    guard status == errSecSuccess, let key = item as? Data, key.count == 32 else {
      throw PremiumAppStateSyncError.keychain(status)
    }
    return key
  }

  private func write(_ key: Data, _ account: String) throws {
    var attributes = baseQuery(account)
    attributes[kSecValueData as String] = key
    attributes[kSecAttrAccessible as String] =
      kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    let status = SecItemAdd(attributes as CFDictionary, nil)
    if status == errSecDuplicateItem {
      let update =
        [
          kSecValueData as String: key,
          kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ] as [String: Any]
      let updateStatus = SecItemUpdate(
        baseQuery(account) as CFDictionary,
        update as CFDictionary
      )
      guard updateStatus == errSecSuccess else {
        throw PremiumAppStateSyncError.keychain(updateStatus)
      }
      return
    }
    guard status == errSecSuccess else {
      throw PremiumAppStateSyncError.keychain(status)
    }
  }

  private func baseQuery(_ account: String) -> [String: Any] {
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

public struct PremiumDecryptedAppUpdate: Equatable, Sendable {
  public let appID: String
  public let recordKey: String
  public let remoteRevision: String
  public let update: Data
}

public struct PremiumAppSyncResult: Equatable, Sendable {
  public let uploaded: Bool
  public let updates: [PremiumDecryptedAppUpdate]
}

public enum PremiumAppStateSyncError: Error, Equatable, Sendable {
  case invalidKey
  case invalidUpdate
  case invalidResponse
  case missingOrganization
  case missingDevice
  case missingKeyGrant
  case keychain(OSStatus)
}

extension PremiumAppStateSyncError: LocalizedError {
  public var errorDescription: String? {
    switch self {
    case .invalidKey:
      return "The app sync key is invalid."
    case .invalidUpdate:
      return "The app sync update is empty or too large."
    case .invalidResponse:
      return "Premium returned an invalid app sync response."
    case .missingOrganization:
      return "The Premium account is not attached to an organization."
    case .missingDevice:
      return "This Premium session is not attached to an active device."
    case .missingKeyGrant:
      return "Approve this connected device before synchronizing app data."
    case .keychain(let status):
      return "App sync could not access Keychain (\(status))."
    }
  }
}

public actor PremiumAppStateSyncClient {
  private struct Organization: Decodable, Sendable {
    let id: String
  }

  private struct Device: Decodable, Sendable {
    let id: String
    let platform: String
    let status: String
  }

  private struct DeviceKey: Decodable, Sendable {
    let deviceId: String
    let publicKey: String
    let fingerprint: String
  }

  private struct DeviceKeysResponse: Decodable, Sendable {
    let devices: [DeviceKey]
  }

  private struct Pairing: Decodable, Sendable {
    let senderDeviceId: String
    let recipientDeviceId: String
    let status: String
  }

  private struct PairingsResponse: Decodable, Sendable {
    let pairings: [Pairing]
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

  private struct Envelope: Codable, Equatable, Sendable {
    let algorithm: String
    let keyId: String
    let iv: String
    let ciphertext: String
    let authTag: String
  }

  private struct WrappedKeyEnvelope: Codable, Equatable, Sendable {
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
    let keyId: String
    let recipientDeviceId: String
    let wrappedKeyEnvelope: WrappedKeyEnvelope
  }

  private struct KeyGrant: Decodable, Sendable {
    let contract: String
    let orgId: String
    let keyId: String
    let senderDeviceId: String
    let recipientDeviceId: String
    let senderPublicKey: String
    let senderKeyFingerprint: String
    let recipientKeyFingerprint: String
    let wrappedKeyEnvelope: WrappedKeyEnvelope?
  }

  private struct GrantResponse: Decodable, Sendable {
    let grant: KeyGrant
    let idempotent: Bool?
  }

  private struct SyncActor: Codable, Equatable, Sendable {
    let subject: String
  }

  private struct SyncRecordRequest: Encodable, Sendable {
    let recordType: String
    let recordKey: String
    let appId: String
    let actor: SyncActor
    let baseRemoteRevision: String?
    let encryptedPayload: Envelope
    let metadata: [String: String]
  }

  private struct PushRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let records: [SyncRecordRequest]
  }

  private struct AcceptedRecord: Decodable, Sendable {
    let recordKey: String
    let remoteRevision: String
  }

  private struct PushResponse: Decodable, Sendable {
    let accepted: [AcceptedRecord]
    let conflicts: [SyncConflict]
  }

  private struct SyncConflict: Decodable, Sendable {
    let id: String
  }

  private struct PullRequest: Encodable, Sendable {
    let orgId: String
    let deviceId: String
    let appId: String
  }

  private struct PulledRecord: Decodable, Sendable {
    let recordType: String
    let recordKey: String
    let appId: String?
    let remoteRevision: String
    let encryptedPayload: Envelope
  }

  private struct PullResponse: Decodable, Sendable {
    let records: [PulledRecord]
    let cursor: String
  }

  private let session: PremiumSessionClient
  private let keyStore: any PremiumAppSyncKeyStore
  private let deviceKeyStore: any PremiumHealthDeviceKeyStore
  private let platform: PremiumPlatform

  public init(
    session: PremiumSessionClient,
    keyStore: any PremiumAppSyncKeyStore = PremiumKeychainAppSyncKeyStore(),
    deviceKeyStore: any PremiumHealthDeviceKeyStore =
      PremiumKeychainHealthDeviceKeyStore(),
    platform: PremiumPlatform
  ) {
    self.session = session
    self.keyStore = keyStore
    self.deviceKeyStore = deviceKeyStore
    self.platform = platform
  }

  public func currentDeviceID() async throws -> String {
    try await resolveContext().deviceID
  }

  public func sync(appID: String, localUpdate: Data?) async throws -> PremiumAppSyncResult {
    guard Self.isSafeIdentifier(appID) else {
      throw PremiumAppStateSyncError.invalidUpdate
    }
    let context = try await resolveContext()
    let key = try await keyStore.loadOrCreateKey()
    let keyID = premiumAppSyncKeyID(key)
    try await grantKeyToApprovedPeers(
      key,
      keyID: keyID,
      context: context
    )
    var uploaded = false
    var collidedUpload: (recordKey: String, update: Data)?
    if let localUpdate, !localUpdate.isEmpty {
      guard localUpdate.count <= 8 * 1024 * 1024 else {
        throw PremiumAppStateSyncError.invalidUpdate
      }
      let recordKey = Self.recordKey(
        appID: appID,
        senderDeviceID: context.deviceID,
        update: localUpdate
      )
      let envelope = try Self.seal(
        localUpdate,
        key: key,
        keyID: keyID,
        authenticatedData: Self.recordContext(
          orgID: context.orgID,
          appID: appID,
          recordKey: recordKey
        )
      )
      do {
        let response: PushResponse = try await session.send(
          path: "sync/push",
          method: .post,
          body: PushRequest(
            orgId: context.orgID,
            deviceId: context.deviceID,
            records: [
              SyncRecordRequest(
                recordType: "crdt.update",
                recordKey: recordKey,
                appId: appID,
                actor: SyncActor(subject: "device:\(context.deviceID)"),
                baseRemoteRevision: nil,
                encryptedPayload: envelope,
                metadata: ["contract": premiumAppStateSyncContract]
              )
            ]
          )
        )
        if response.conflicts.isEmpty {
          guard response.accepted.count == 1,
            response.accepted[0].recordKey == recordKey
          else {
            throw PremiumAppStateSyncError.invalidResponse
          }
          uploaded = true
        } else {
          guard response.accepted.isEmpty, response.conflicts.count == 1 else {
            throw PremiumAppStateSyncError.invalidResponse
          }
          collidedUpload = (recordKey, localUpdate)
        }
      } catch let error as PremiumSessionError {
        guard case .server(let statusCode, _) = error, statusCode == 409 else {
          throw error
        }
        collidedUpload = (recordKey, localUpdate)
      } catch {
        throw error
      }
    }

    let pulled: PullResponse = try await session.send(
      path: "sync/pull",
      method: .post,
      body: PullRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        appId: appID
      )
    )
    let updates = try await pulled.records.mapAsync { record in
      guard record.recordType == "crdt.update", record.appId == appID else {
        throw PremiumAppStateSyncError.invalidResponse
      }
      let key = try await self.resolveKey(
        id: record.encryptedPayload.keyId,
        context: context
      )
      let update = try Self.open(
        record.encryptedPayload,
        key: key,
        authenticatedData: Self.recordContext(
          orgID: context.orgID,
          appID: appID,
          recordKey: record.recordKey
        )
      )
      guard !update.isEmpty else {
        throw PremiumAppStateSyncError.invalidResponse
      }
      guard
        Self.recordKeyMatches(
          record.recordKey,
          appID: appID,
          update: update
        )
      else {
        throw PremiumAppStateSyncError.invalidResponse
      }
      return PremiumDecryptedAppUpdate(
        appID: appID,
        recordKey: record.recordKey,
        remoteRevision: record.remoteRevision,
        update: update
      )
    }
    if let collidedUpload,
      !Self.collisionMatches(
        recordKey: collidedUpload.recordKey,
        localUpdate: collidedUpload.update,
        updates: updates
      )
    {
      throw PremiumAppStateSyncError.invalidResponse
    }
    return PremiumAppSyncResult(uploaded: uploaded, updates: updates)
  }

  private func resolveContext() async throws -> (orgID: String, deviceID: String) {
    let organizations: [Organization] = try await session.send(path: "users/me/orgs")
    guard let orgID = organizations.first?.id else {
      throw PremiumAppStateSyncError.missingOrganization
    }
    let devices: [Device] = try await session.send(path: "users/me/devices")
    let sessionDeviceID = await session.currentDeviceID
    guard
      let deviceID =
        sessionDeviceID
        ?? devices.first(where: {
          $0.platform == platform.rawValue && $0.status == "active"
        })?.id
    else {
      throw PremiumAppStateSyncError.missingDevice
    }
    return (orgID, deviceID)
  }

  private func registerDeviceKey(
    context: (orgID: String, deviceID: String)
  ) async throws -> (Curve25519.KeyAgreement.PrivateKey, String) {
    let raw = try await deviceKeyStore.loadOrCreatePrivateKey()
    let privateKey = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: raw)
    let publicKey = privateKey.publicKey.rawRepresentation
    let registered: RegisteredDeviceKey = try await session.send(
      path: "sync/device-keys",
      method: .post,
      body: RegisterDeviceKeyRequest(
        orgId: context.orgID,
        deviceId: context.deviceID,
        publicKey: Self.base64URL(publicKey)
      )
    )
    let fingerprint = Self.fingerprint(publicKey)
    guard registered.deviceId == context.deviceID,
      registered.publicKey == Self.base64URL(publicKey),
      registered.fingerprint == fingerprint
    else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    return (privateKey, fingerprint)
  }

  private func grantKeyToApprovedPeers(
    _ key: Data,
    keyID: String,
    context: (orgID: String, deviceID: String)
  ) async throws {
    let (_, senderFingerprint) = try await registerDeviceKey(context: context)
    let keys = try await listDeviceKeys(context: context)
    let pairings = try await listPairings(context: context)
    for pairing in pairings.pairings
    where pairing.status == "approved"
      && pairing.senderDeviceId == context.deviceID
      && pairing.recipientDeviceId != context.deviceID
    {
      guard
        let peer = keys.devices.first(where: {
          $0.deviceId == pairing.recipientDeviceId
        }),
        peer.fingerprint == Self.fingerprint(try Self.decodeBase64URL(peer.publicKey))
      else {
        throw PremiumAppStateSyncError.invalidResponse
      }
      let wrapped = try Self.wrapKey(
        key,
        recipientPublicKey: try Self.decodeBase64URL(peer.publicKey),
        recipientFingerprint: peer.fingerprint,
        orgID: context.orgID,
        keyID: keyID,
        senderDeviceID: context.deviceID,
        recipientDeviceID: peer.deviceId,
        senderFingerprint: senderFingerprint
      )
      let response: GrantResponse = try await session.send(
        path: "sync/data-key-grants",
        method: .post,
        body: GrantKeyRequest(
          orgId: context.orgID,
          deviceId: context.deviceID,
          keyId: keyID,
          recipientDeviceId: peer.deviceId,
          wrappedKeyEnvelope: wrapped
        )
      )
      guard response.grant.keyId == keyID,
        response.grant.senderDeviceId == context.deviceID,
        response.grant.recipientDeviceId == peer.deviceId
      else {
        throw PremiumAppStateSyncError.invalidResponse
      }
    }
  }

  private func resolveKey(
    id: String,
    context: (orgID: String, deviceID: String)
  ) async throws -> Data {
    if let key = try await keyStore.loadKey(id: id) {
      return key
    }
    let (privateKey, recipientFingerprint) = try await registerDeviceKey(
      context: context)
    var components = URLComponents()
    components.path = "sync/data-key-grants/\(id)"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let response: GrantResponse
    do {
      response = try await session.send(path: path)
    } catch let error as PremiumSessionError {
      if case .server(let status, _) = error, status == 404 {
        throw PremiumAppStateSyncError.missingKeyGrant
      }
      throw error
    }
    let grant = response.grant
    guard grant.contract == "terrane.sync-data-key-grant.v1",
      grant.orgId == context.orgID,
      grant.keyId == id,
      grant.recipientDeviceId == context.deviceID,
      grant.recipientKeyFingerprint == recipientFingerprint,
      grant.senderKeyFingerprint
        == Self.fingerprint(try Self.decodeBase64URL(grant.senderPublicKey)),
      let wrapped = grant.wrappedKeyEnvelope
    else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let key = try Self.unwrapKey(
      wrapped,
      privateKey: privateKey,
      recipientFingerprint: recipientFingerprint,
      orgID: context.orgID,
      keyID: id,
      senderDeviceID: grant.senderDeviceId,
      recipientDeviceID: context.deviceID,
      senderFingerprint: grant.senderKeyFingerprint
    )
    guard key.count == 32, premiumAppSyncKeyID(key) == id else {
      throw PremiumAppStateSyncError.invalidKey
    }
    try await keyStore.saveKey(key, id: id)
    return key
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
      throw PremiumAppStateSyncError.invalidResponse
    }
    return try await session.send(path: path)
  }

  private func listPairings(
    context: (orgID: String, deviceID: String)
  ) async throws -> PairingsResponse {
    var components = URLComponents()
    components.path = "sync/device-key-pairings"
    components.queryItems = [
      URLQueryItem(name: "orgId", value: context.orgID),
      URLQueryItem(name: "deviceId", value: context.deviceID),
    ]
    guard let path = components.string else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    return try await session.send(path: path)
  }

  static func recordKey(
    appID: String,
    senderDeviceID: String,
    update: Data
  ) -> String {
    "\(appID):\(senderDeviceID):\(premiumAppSyncUpdateID(update))"
  }

  static func recordKeyMatches(
    _ recordKey: String,
    appID: String,
    update: Data
  ) -> Bool {
    let parts = recordKey.split(separator: ":", omittingEmptySubsequences: false)
    guard parts.count == 3,
      parts[0] == Substring(appID),
      !parts[1].isEmpty
    else {
      return false
    }
    return parts[2] == Substring(premiumAppSyncUpdateID(update))
  }

  static func collisionMatches(
    recordKey: String,
    localUpdate: Data,
    updates: [PremiumDecryptedAppUpdate]
  ) -> Bool {
    updates.contains {
      $0.recordKey == recordKey
        && $0.update == localUpdate
        && recordKeyMatches(recordKey, appID: $0.appID, update: $0.update)
    }
  }

  private static func seal(
    _ plaintext: Data,
    key: Data,
    keyID: String,
    authenticatedData: Data
  ) throws -> Envelope {
    guard key.count == 32 else {
      throw PremiumAppStateSyncError.invalidKey
    }
    let sealed = try AES.GCM.seal(
      plaintext,
      using: SymmetricKey(data: key),
      authenticating: authenticatedData
    )
    return Envelope(
      algorithm: "aes-256-gcm",
      keyId: keyID,
      iv: base64URL(Data(sealed.nonce)),
      ciphertext: base64URL(sealed.ciphertext),
      authTag: base64URL(sealed.tag)
    )
  }

  private static func open(
    _ envelope: Envelope,
    key: Data,
    authenticatedData: Data
  ) throws -> Data {
    guard envelope.algorithm == "aes-256-gcm",
      envelope.keyId == premiumAppSyncKeyID(key)
    else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let box = try AES.GCM.SealedBox(
      nonce: AES.GCM.Nonce(data: try decodeBase64URL(envelope.iv)),
      ciphertext: try decodeBase64URL(envelope.ciphertext),
      tag: try decodeBase64URL(envelope.authTag)
    )
    return try AES.GCM.open(
      box,
      using: SymmetricKey(data: key),
      authenticating: authenticatedData
    )
  }

  private static func wrapKey(
    _ key: Data,
    recipientPublicKey: Data,
    recipientFingerprint: String,
    orgID: String,
    keyID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderFingerprint: String
  ) throws -> WrappedKeyEnvelope {
    let ephemeral = Curve25519.KeyAgreement.PrivateKey()
    let recipient = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: recipientPublicKey)
    let shared = try ephemeral.sharedSecretFromKeyAgreement(with: recipient)
    let salt = try randomData(count: 32)
    let context = wrappedKeyContext(
      orgID: orgID,
      keyID: keyID,
      senderDeviceID: senderDeviceID,
      recipientDeviceID: recipientDeviceID,
      senderFingerprint: senderFingerprint,
      recipientFingerprint: recipientFingerprint
    )
    let wrappingKey = shared.hkdfDerivedSymmetricKey(
      using: SHA256.self,
      salt: salt,
      sharedInfo: Data("terrane.sync-wrapped-key.v1".utf8),
      outputByteCount: 32
    )
    let sealed = try AES.GCM.seal(
      key,
      using: wrappingKey,
      authenticating: context
    )
    return WrappedKeyEnvelope(
      contract: "terrane.sync-wrapped-key.v1",
      algorithm: "x25519-hkdf-sha256-aes-256-gcm",
      ephemeralPublicKey: base64URL(ephemeral.publicKey.rawRepresentation),
      recipientKeyFingerprint: recipientFingerprint,
      salt: base64URL(salt),
      iv: base64URL(Data(sealed.nonce)),
      ciphertext: base64URL(sealed.ciphertext),
      authTag: base64URL(sealed.tag),
      contextSha256: sha256Identifier(context)
    )
  }

  private static func unwrapKey(
    _ envelope: WrappedKeyEnvelope,
    privateKey: Curve25519.KeyAgreement.PrivateKey,
    recipientFingerprint: String,
    orgID: String,
    keyID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderFingerprint: String
  ) throws -> Data {
    guard envelope.contract == "terrane.sync-wrapped-key.v1",
      envelope.algorithm == "x25519-hkdf-sha256-aes-256-gcm",
      envelope.recipientKeyFingerprint == recipientFingerprint
    else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let context = wrappedKeyContext(
      orgID: orgID,
      keyID: keyID,
      senderDeviceID: senderDeviceID,
      recipientDeviceID: recipientDeviceID,
      senderFingerprint: senderFingerprint,
      recipientFingerprint: recipientFingerprint
    )
    guard envelope.contextSha256 == sha256Identifier(context) else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let ephemeral = try Curve25519.KeyAgreement.PublicKey(
      rawRepresentation: try decodeBase64URL(envelope.ephemeralPublicKey))
    let salt = try decodeBase64URL(envelope.salt)
    guard salt.count == 32 else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    let shared = try privateKey.sharedSecretFromKeyAgreement(with: ephemeral)
    let wrappingKey = shared.hkdfDerivedSymmetricKey(
      using: SHA256.self,
      salt: salt,
      sharedInfo: Data("terrane.sync-wrapped-key.v1".utf8),
      outputByteCount: 32
    )
    let box = try AES.GCM.SealedBox(
      nonce: AES.GCM.Nonce(data: try decodeBase64URL(envelope.iv)),
      ciphertext: try decodeBase64URL(envelope.ciphertext),
      tag: try decodeBase64URL(envelope.authTag)
    )
    return try AES.GCM.open(
      box,
      using: wrappingKey,
      authenticating: context
    )
  }

  private static func recordContext(
    orgID: String,
    appID: String,
    recordKey: String
  ) -> Data {
    Data(
      """
      {"contract":"\(premiumAppStateSyncContract)","orgId":"\(orgID)","appId":"\(appID)","recordKey":"\(recordKey)"}
      """.utf8
    )
  }

  private static func wrappedKeyContext(
    orgID: String,
    keyID: String,
    senderDeviceID: String,
    recipientDeviceID: String,
    senderFingerprint: String,
    recipientFingerprint: String
  ) -> Data {
    Data(
      """
      {"contract":"terrane.sync-data-key-grant.v1","orgId":"\(orgID)","keyId":"\(keyID)","senderDeviceId":"\(senderDeviceID)","recipientDeviceId":"\(recipientDeviceID)","senderKeyFingerprint":"\(senderFingerprint)","recipientKeyFingerprint":"\(recipientFingerprint)"}
      """.utf8
    )
  }

  private static func fingerprint(_ publicKey: Data) -> String {
    sha256Identifier(publicKey)
  }

  private static func sha256Identifier(_ data: Data) -> String {
    "sha256:\(hex(SHA256.hash(data: data)))"
  }

  private static func hex<D: Sequence>(_ data: D) -> String where D.Element == UInt8 {
    data.map { String(format: "%02x", $0) }.joined()
  }

  private static func randomData(count: Int) throws -> Data {
    var bytes = [UInt8](repeating: 0, count: count)
    guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    return Data(bytes)
  }

  private static func base64URL(_ data: Data) -> String {
    data.base64EncodedString()
      .replacingOccurrences(of: "+", with: "-")
      .replacingOccurrences(of: "/", with: "_")
      .replacingOccurrences(of: "=", with: "")
  }

  private static func decodeBase64URL(_ value: String) throws -> Data {
    var normalized = value.replacingOccurrences(of: "-", with: "+")
      .replacingOccurrences(of: "_", with: "/")
    normalized.append(String(repeating: "=", count: (4 - normalized.count % 4) % 4))
    guard let data = Data(base64Encoded: normalized) else {
      throw PremiumAppStateSyncError.invalidResponse
    }
    return data
  }

  private static func isSafeIdentifier(_ value: String) -> Bool {
    guard !value.isEmpty, value.count <= 128 else { return false }
    return value.allSatisfy {
      $0.isLetter || $0.isNumber || "._:-".contains($0)
    }
  }
}

private func premiumAppSyncKeyID(_ key: Data) -> String {
  "sync-\(SHA256.hash(data: key).prefix(12).map { String(format: "%02x", $0) }.joined())"
}

extension Sequence {
  fileprivate func mapAsync<T>(
    _ transform: (Element) async throws -> T
  ) async rethrows -> [T] {
    var values: [T] = []
    for element in self {
      values.append(try await transform(element))
    }
    return values
  }
}
