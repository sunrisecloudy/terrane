import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

public protocol PremiumAccountLifecycleHooks: Sendable {
  func providerLinkingWillBegin(_ provider: PremiumIdentityProvider) async
  func providerLinkingDidFinish(_ provider: PremiumIdentityProvider, succeeded: Bool) async
  func accountDeletionWillBegin() async
  func accountDeletionDidFinish(succeeded: Bool) async
}

public struct NoopPremiumAccountLifecycleHooks: PremiumAccountLifecycleHooks {
  public init() {}

  public func providerLinkingWillBegin(_ provider: PremiumIdentityProvider) async {}
  public func providerLinkingDidFinish(
    _ provider: PremiumIdentityProvider, succeeded: Bool
  ) async {}
  public func accountDeletionWillBegin() async {}
  public func accountDeletionDidFinish(succeeded: Bool) async {}
}

public actor PremiumSessionClient {
  public typealias StateObserver = @Sendable (PremiumSessionState) -> Void

  private struct ChallengeRequest: Encodable {
    let provider: PremiumIdentityProvider
    let platform: PremiumPlatform
    let deviceName: String
    let clientVersion: String
  }

  private struct AppleExchangeRequest: Encodable, Sendable {
    let challengeId: String
    let identityToken: String
    let authorizationCode: String
    let displayName: String?
  }

  private struct GoogleExchangeRequest: Encodable, Sendable {
    let challengeId: String
    let idToken: String
  }

  private struct RefreshRequest: Encodable {
    let refreshToken: String
  }

  private struct SessionEnvelope: Decodable, Sendable {
    let accessToken: String
    let refreshToken: String
    let expiresAt: Date?
    let account: PremiumAccount?

    private enum CodingKeys: String, CodingKey {
      case session
      case user
      case account
      case accessToken
      case refreshToken
      case expiresIn
      case expiresAt
    }

    private struct ServerSession: Decodable {
      let token: String
      let refreshToken: String
      let userId: String
      let authMethod: PremiumIdentityProvider?
      let accessExpiresAt: Date?
    }

    init(from decoder: Decoder) throws {
      let container = try decoder.container(keyedBy: CodingKeys.self)
      if let session = try container.decodeIfPresent(ServerSession.self, forKey: .session) {
        accessToken = session.token
        refreshToken = session.refreshToken
        expiresAt = session.accessExpiresAt
        let decodedAccount =
          try container.decodeIfPresent(PremiumAccount.self, forKey: .account)
          ?? container.decodeIfPresent(PremiumAccount.self, forKey: .user)
        account =
          decodedAccount
          ?? PremiumAccount(
            id: session.userId,
            linkedProviders: session.authMethod.map { [$0] } ?? []
          )
        return
      }
      accessToken = try container.decode(String.self, forKey: .accessToken)
      refreshToken = try container.decode(String.self, forKey: .refreshToken)
      let expiresIn = try container.decodeIfPresent(TimeInterval.self, forKey: .expiresIn)
      expiresAt =
        try container.decodeIfPresent(Date.self, forKey: .expiresAt)
        ?? expiresIn.map { Date().addingTimeInterval($0) }
      account =
        try container.decodeIfPresent(PremiumAccount.self, forKey: .account)
        ?? container.decodeIfPresent(PremiumAccount.self, forKey: .user)
    }
  }

  private struct APIEnvelope<Value: Decodable>: Decodable {
    let ok: Bool
    let result: Value?
  }

  private struct AccessCredential: Sendable {
    let token: String
    let expiresAt: Date?

    func isUsable(at date: Date, leeway: TimeInterval) -> Bool {
      guard let expiresAt else { return true }
      return expiresAt.timeIntervalSince(date) > leeway
    }
  }

  public private(set) var state: PremiumSessionState = .signedOut {
    didSet { stateObserver?(state) }
  }

  private let baseURL: URL
  private let device: PremiumDeviceMetadata
  private let tokenStore: any PremiumRefreshTokenStore
  private let transport: any PremiumHTTPTransport
  private let lifecycleHooks: any PremiumAccountLifecycleHooks
  private let stateObserver: StateObserver?
  private let now: @Sendable () -> Date
  private let encoder: JSONEncoder
  private let decoder: JSONDecoder
  private var accessCredential: AccessCredential?
  private var account: PremiumAccount?
  private var refreshTask: Task<SessionEnvelope, Error>?

  public init(
    baseURL: URL,
    device: PremiumDeviceMetadata,
    tokenStore: any PremiumRefreshTokenStore = PremiumKeychainRefreshTokenStore(),
    transport: any PremiumHTTPTransport = PremiumURLSessionTransport(),
    lifecycleHooks: any PremiumAccountLifecycleHooks = NoopPremiumAccountLifecycleHooks(),
    stateObserver: StateObserver? = nil,
    now: @escaping @Sendable () -> Date = { Date() }
  ) throws {
    guard let scheme = baseURL.scheme?.lowercased(), let host = baseURL.host,
      scheme == "https" || (scheme == "http" && Self.isLoopbackHost(host))
    else {
      throw PremiumSessionError.invalidBaseURL
    }
    self.baseURL = baseURL
    self.device = device
    self.tokenStore = tokenStore
    self.transport = transport
    self.lifecycleHooks = lifecycleHooks
    self.stateObserver = stateObserver
    self.now = now
    encoder = JSONEncoder()
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601
    self.decoder = decoder
  }

  /// Restores a persisted refresh token without making sign-in a prerequisite
  /// for local Terrane operation. A missing token is a normal signed-out state.
  public func restoreSession() async {
    do {
      guard try await tokenStore.read() != nil else {
        transitionToSignedOut()
        return
      }
      _ = try await refresh(force: true)
    } catch {
      await handleRefreshFailure(error)
    }
  }

  public func beginAuthentication(
    provider: PremiumIdentityProvider
  ) async throws -> PremiumAuthenticationChallenge {
    if case .authenticating = state {
      throw PremiumSessionError.authenticationInProgress
    }
    state = .authenticating(PremiumAuthenticationContext(provider: provider))
    do {
      let challenge: PremiumAuthenticationChallenge = try await sendUnauthenticated(
        path: "auth/native/challenge",
        body: ChallengeRequest(
          provider: provider,
          platform: device.platform,
          deviceName: device.deviceName,
          clientVersion: device.clientVersion
        )
      )
      state = .authenticating(
        PremiumAuthenticationContext(provider: provider, challengeId: challenge.challengeId))
      return challenge
    } catch {
      transitionAfterInteractiveAuthenticationFailure(error)
      throw error
    }
  }

  /// Cancels a host-owned provider sheet without disturbing an existing
  /// signed-in account. This is used by native Apple/Google UI when the user
  /// dismisses authorization before an exchange occurs.
  public func cancelAuthentication() {
    guard case .authenticating = state else { return }
    state = account.map(PremiumSessionState.signedIn) ?? .signedOut
  }

  @discardableResult
  public func exchangeAppleCredential(
    challengeId: String,
    credential: PremiumAppleCredential
  ) async throws -> PremiumAccount {
    try requireChallenge(challengeId, provider: .apple)
    do {
      let session: SessionEnvelope = try await sendUnauthenticated(
        path: "auth/apple/native/exchange",
        body: AppleExchangeRequest(
          challengeId: challengeId,
          identityToken: credential.identityToken,
          authorizationCode: credential.authorizationCode,
          displayName: credential.displayName
        )
      )
      return try await install(session)
    } catch {
      transitionAfterInteractiveAuthenticationFailure(error)
      throw error
    }
  }

  @discardableResult
  public func exchangeGoogleCredential(
    challengeId: String,
    credential: PremiumGoogleCredential
  ) async throws -> PremiumAccount {
    try requireChallenge(challengeId, provider: .google)
    do {
      let session: SessionEnvelope = try await sendUnauthenticated(
        path: "auth/google/native/exchange",
        body: GoogleExchangeRequest(challengeId: challengeId, idToken: credential.idToken)
      )
      return try await install(session)
    } catch {
      transitionAfterInteractiveAuthenticationFailure(error)
      throw error
    }
  }

  /// Links Apple to the current account by using the native exchange endpoint
  /// with host-owned bearer authentication. No credential or token crosses the
  /// generated-app bridge.
  @discardableResult
  public func linkAppleCredential(
    challengeId: String,
    credential: PremiumAppleCredential
  ) async throws -> PremiumAccount {
    try requireChallenge(challengeId, provider: .apple)
    guard account != nil, accessCredential != nil else {
      throw PremiumSessionError.notAuthenticated
    }
    await lifecycleHooks.providerLinkingWillBegin(.apple)
    do {
      let session: SessionEnvelope = try await send(
        path: "auth/apple/native/exchange",
        method: .post,
        body: AppleExchangeRequest(
          challengeId: challengeId,
          identityToken: credential.identityToken,
          authorizationCode: credential.authorizationCode,
          displayName: credential.displayName
        ),
        response: SessionEnvelope.self
      )
      let linkedAccount = try await install(session)
      await lifecycleHooks.providerLinkingDidFinish(.apple, succeeded: true)
      return linkedAccount
    } catch {
      await lifecycleHooks.providerLinkingDidFinish(.apple, succeeded: false)
      throw error
    }
  }

  /// Links Google to the current account using the same authenticated exchange
  /// contract as the Premium server.
  @discardableResult
  public func linkGoogleCredential(
    challengeId: String,
    credential: PremiumGoogleCredential
  ) async throws -> PremiumAccount {
    try requireChallenge(challengeId, provider: .google)
    guard account != nil, accessCredential != nil else {
      throw PremiumSessionError.notAuthenticated
    }
    await lifecycleHooks.providerLinkingWillBegin(.google)
    do {
      let session: SessionEnvelope = try await send(
        path: "auth/google/native/exchange",
        method: .post,
        body: GoogleExchangeRequest(challengeId: challengeId, idToken: credential.idToken),
        response: SessionEnvelope.self
      )
      let linkedAccount = try await install(session)
      await lifecycleHooks.providerLinkingDidFinish(.google, succeeded: true)
      return linkedAccount
    } catch {
      await lifecycleHooks.providerLinkingDidFinish(.google, succeeded: false)
      throw error
    }
  }

  @discardableResult
  public func refresh(force: Bool = false) async throws -> PremiumAccount {
    if !force, let credential = accessCredential,
      credential.isUsable(at: now(), leeway: 30), let account
    {
      return account
    }
    if let refreshTask {
      let session = try await refreshTask.value
      return try await install(session)
    }
    state = .refreshing(account)
    let tokenStore = self.tokenStore
    let baseURL = self.baseURL
    let transport = self.transport
    let encoder = self.encoder
    let decoder = self.decoder
    let task = Task {
      guard let refreshToken = try await tokenStore.read(), !refreshToken.isEmpty else {
        throw PremiumSessionError.missingRefreshToken
      }
      return try await Self.performRefresh(
        baseURL: baseURL,
        refreshToken: refreshToken,
        transport: transport,
        encoder: encoder,
        decoder: decoder
      )
    }
    refreshTask = task
    do {
      let session = try await task.value
      refreshTask = nil
      return try await install(session)
    } catch {
      refreshTask = nil
      await handleRefreshFailure(error)
      throw error
    }
  }

  /// Sends an authenticated Premium API request. The access token stays inside
  /// this actor and a 401 is retried once after the shared refresh flight.
  public func send<Response: Decodable & Sendable>(
    path: String,
    method: PremiumHTTPMethod = .get,
    body: Data? = nil,
    response: Response.Type = Response.self
  ) async throws -> Response {
    let credential = try await validAccessCredential()
    let request = try makeRequest(
      path: path, method: method, body: body, bearerToken: credential.token)
    let (data, httpResponse) = try await perform(request)
    if httpResponse.statusCode == 401 {
      _ = try await refresh(force: true)
      guard let retryCredential = accessCredential else {
        throw PremiumSessionError.notAuthenticated
      }
      let retry = try makeRequest(
        path: path, method: method, body: body, bearerToken: retryCredential.token)
      let (retryData, retryResponse) = try await perform(retry)
      try Self.validate(retryResponse, data: retryData)
      return try decode(Response.self, from: retryData)
    }
    try Self.validate(httpResponse, data: data)
    return try decode(Response.self, from: data)
  }

  public func send<Body: Encodable & Sendable, Response: Decodable & Sendable>(
    path: String,
    method: PremiumHTTPMethod,
    body: Body,
    response: Response.Type = Response.self
  ) async throws -> Response {
    let encodedBody: Data? = try encoder.encode(body)
    return try await send(
      path: path,
      method: method,
      body: encodedBody,
      response: response
    )
  }

  public func encodeRequestBody<Body: Encodable>(_ body: Body) throws -> Data {
    try encoder.encode(body)
  }

  /// Clears local credentials even when the server is unreachable. The logout
  /// request uses a snapshot of the in-memory access token and never exposes it
  /// to callers or WebKit.
  public func logout() async throws {
    let token = accessCredential?.token
    transitionToSignedOut()
    try await tokenStore.delete()
    guard let token else { return }
    let request = try makeRequest(
      path: "account/session/logout", method: .post, body: nil, bearerToken: token)
    let (data, response) = try await perform(request)
    try Self.validate(response, data: data)
  }

  public func withProviderLinking(
    _ provider: PremiumIdentityProvider,
    operation: @Sendable () async throws -> Void
  ) async throws {
    guard account != nil, accessCredential != nil else {
      throw PremiumSessionError.notAuthenticated
    }
    await lifecycleHooks.providerLinkingWillBegin(provider)
    do {
      try await operation()
      await lifecycleHooks.providerLinkingDidFinish(provider, succeeded: true)
    } catch {
      await lifecycleHooks.providerLinkingDidFinish(provider, succeeded: false)
      throw error
    }
  }

  /// Wraps the server-specific deletion call. Successful deletion always
  /// removes the refresh token and transitions the client to `revoked`.
  public func withAccountDeletion(
    operation: @Sendable () async throws -> Void
  ) async throws {
    guard account != nil, accessCredential != nil else {
      throw PremiumSessionError.notAuthenticated
    }
    await lifecycleHooks.accountDeletionWillBegin()
    do {
      try await operation()
    } catch {
      await lifecycleHooks.accountDeletionDidFinish(succeeded: false)
      throw error
    }
    accessCredential = nil
    account = nil
    state = .revoked
    do {
      try await tokenStore.delete()
    } catch {
      await lifecycleHooks.accountDeletionDidFinish(succeeded: true)
      throw error
    }
    await lifecycleHooks.accountDeletionDidFinish(succeeded: true)
  }

  private func requireChallenge(
    _ challengeId: String, provider: PremiumIdentityProvider
  ) throws {
    guard
      case .authenticating(let context) = state,
      context.provider == provider,
      context.challengeId == challengeId
    else {
      throw PremiumSessionError.notAuthenticated
    }
  }

  private func validAccessCredential() async throws -> AccessCredential {
    if let credential = accessCredential, credential.isUsable(at: now(), leeway: 30) {
      return credential
    }
    _ = try await refresh(force: true)
    guard let credential = accessCredential else {
      throw PremiumSessionError.notAuthenticated
    }
    return credential
  }

  private func install(_ session: SessionEnvelope) async throws -> PremiumAccount {
    guard let resolvedAccount = account ?? session.account else {
      throw PremiumSessionError.invalidResponse
    }
    try await tokenStore.save(session.refreshToken)
    accessCredential = AccessCredential(token: session.accessToken, expiresAt: session.expiresAt)
    account = resolvedAccount
    state = .signedIn(resolvedAccount)
    return resolvedAccount
  }

  private func transitionToSignedOut() {
    refreshTask?.cancel()
    refreshTask = nil
    accessCredential = nil
    account = nil
    state = .signedOut
  }

  private func transitionAfterInteractiveAuthenticationFailure(_ error: Error) {
    accessCredential = nil
    if Self.isConnectivityError(error) {
      state = .offline(PremiumOfflineContext(account: account, message: error.localizedDescription))
    } else {
      state = .signedOut
    }
  }

  private func handleRefreshFailure(_ error: Error) async {
    accessCredential = nil
    if error as? PremiumSessionError == .missingRefreshToken {
      account = nil
      state = .signedOut
    } else if Self.isRevocation(error) {
      account = nil
      try? await tokenStore.delete()
      state = .revoked
    } else if Self.isConnectivityError(error) {
      state = .offline(PremiumOfflineContext(account: account, message: error.localizedDescription))
    } else {
      state = .offline(PremiumOfflineContext(account: account, message: error.localizedDescription))
    }
  }

  private func sendUnauthenticated<Body: Encodable, Response: Decodable>(
    path: String, body: Body
  ) async throws -> Response {
    let data = try encoder.encode(body)
    let request = try makeRequest(path: path, method: .post, body: data, bearerToken: nil)
    let (responseData, response) = try await perform(request)
    try Self.validate(response, data: responseData)
    return try decode(Response.self, from: responseData)
  }

  private func perform(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
    do {
      return try await transport.data(for: request)
    } catch let error as PremiumSessionError {
      throw error
    } catch {
      throw PremiumSessionError.transport(error.localizedDescription)
    }
  }

  private func makeRequest(
    path: String,
    method: PremiumHTTPMethod,
    body: Data?,
    bearerToken: String?
  ) throws -> URLRequest {
    guard !path.isEmpty, !path.hasPrefix("/"), !path.contains("://"),
      let url = URL(string: path, relativeTo: baseURL.appendingPathComponent(""))?.absoluteURL,
      url.scheme == baseURL.scheme,
      url.host == baseURL.host
    else {
      throw PremiumSessionError.invalidPath
    }
    var request = URLRequest(url: url)
    request.httpMethod = method.rawValue
    request.httpBody = body
    request.setValue("application/json", forHTTPHeaderField: "Accept")
    if body != nil {
      request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    }
    if let bearerToken {
      request.setValue("Bearer \(bearerToken)", forHTTPHeaderField: "Authorization")
    }
    return request
  }

  private func decode<Response: Decodable>(_ type: Response.Type, from data: Data) throws
    -> Response
  {
    if type == PremiumEmptyResponse.self, data.isEmpty {
      return PremiumEmptyResponse() as! Response
    }
    do {
      let envelope = try decoder.decode(APIEnvelope<Response>.self, from: data)
      guard envelope.ok, let result = envelope.result else {
        throw PremiumSessionError.invalidResponse
      }
      return result
    } catch let error as PremiumSessionError {
      throw error
    } catch {
      throw PremiumSessionError.invalidResponse
    }
  }

  private static func performRefresh(
    baseURL: URL,
    refreshToken: String,
    transport: any PremiumHTTPTransport,
    encoder: JSONEncoder,
    decoder: JSONDecoder
  ) async throws -> SessionEnvelope {
    let url = baseURL.appendingPathComponent("account/session/refresh")
    var request = URLRequest(url: url)
    request.httpMethod = PremiumHTTPMethod.post.rawValue
    request.httpBody = try encoder.encode(RefreshRequest(refreshToken: refreshToken))
    request.setValue("application/json", forHTTPHeaderField: "Accept")
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    let data: Data
    let response: HTTPURLResponse
    do {
      (data, response) = try await transport.data(for: request)
    } catch {
      throw PremiumSessionError.transport(error.localizedDescription)
    }
    try validate(response, data: data)
    do {
      let envelope = try decoder.decode(APIEnvelope<SessionEnvelope>.self, from: data)
      guard envelope.ok, let session = envelope.result else {
        throw PremiumSessionError.invalidResponse
      }
      return session
    } catch let error as PremiumSessionError {
      throw error
    } catch {
      throw PremiumSessionError.invalidResponse
    }
  }

  private static func validate(_ response: HTTPURLResponse, data: Data) throws {
    guard (200..<300).contains(response.statusCode) else {
      let message = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])
        .flatMap { root -> String? in
          if let error = root["error"] as? [String: Any] {
            return error["message"] as? String
          }
          return (root["message"] as? String) ?? (root["error"] as? String)
        }
      throw PremiumSessionError.server(statusCode: response.statusCode, message: message)
    }
  }

  private static func isRevocation(_ error: Error) -> Bool {
    guard case .server(let status, _) = error as? PremiumSessionError else { return false }
    return status == 401 || status == 403
  }

  private static func isConnectivityError(_ error: Error) -> Bool {
    if case .transport = error as? PremiumSessionError {
      return true
    }
    return false
  }

  private static func isLoopbackHost(_ host: String) -> Bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
  }
}
