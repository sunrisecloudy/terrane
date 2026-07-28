import Foundation
import XCTest

@testable import TerranePremiumSession

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

final class PremiumSessionClientTests: XCTestCase {
  private let baseURL = URL(string: "https://premium.example.test/api/")!
  private let device = PremiumDeviceMetadata(
    platform: .macOS,
    deviceName: "Test Mac",
    clientVersion: "1.2.3"
  )

  func testChallengeAndAppleExchangeMatchNativeContract() async throws {
    let tokenStore = MemoryTokenStore()
    let transport = RecordingTransport { request in
      switch request.url?.path {
      case "/api/auth/native/challenge":
        return Self.response(
          request,
          status: 200,
          json:
            #"{"challengeId":"challenge-1","provider":"apple","nonce":"raw-nonce","nonceSha256":"hashed-nonce","expiresAt":"2026-07-28T12:00:00Z"}"#
        )
      case "/api/auth/apple/native/exchange":
        return Self.response(request, status: 200, json: Self.sessionJSON)
      default:
        return Self.response(request, status: 404, json: #"{"message":"not found"}"#)
      }
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)

    let challenge = try await client.beginAuthentication(provider: .apple)
    XCTAssertEqual(challenge.challengeId, "challenge-1")
    XCTAssertEqual(challenge.nonce, "raw-nonce")
    XCTAssertEqual(challenge.nonceSha256, "hashed-nonce")
    let account = try await client.exchangeAppleCredential(
      challengeId: challenge.challengeId,
      credential: PremiumAppleCredential(
        identityToken: "apple-identity",
        authorizationCode: "apple-code",
        displayName: "Test Person"
      )
    )

    XCTAssertEqual(account.id, "account-1")
    let savedToken = await tokenStore.value()
    let state = await client.state
    XCTAssertEqual(savedToken, "refresh-1")
    XCTAssertEqual(state, .signedIn(account))

    let requests = await transport.recordedRequests()
    XCTAssertEqual(requests.count, 2)
    XCTAssertNil(requests[0].value(forHTTPHeaderField: "Authorization"))
    XCTAssertEqual(
      try Self.jsonObject(requests[0]),
      [
        "provider": "apple",
        "platform": "macos",
        "deviceName": "Test Mac",
        "clientVersion": "1.2.3",
      ]
    )
    XCTAssertEqual(
      try Self.jsonObject(requests[1]),
      [
        "challengeId": "challenge-1",
        "identityToken": "apple-identity",
        "authorizationCode": "apple-code",
        "displayName": "Test Person",
      ]
    )
  }

  func testGoogleExchangeUsesGoogleContract() async throws {
    let tokenStore = MemoryTokenStore()
    let transport = RecordingTransport { request in
      if request.url?.path.hasSuffix("/challenge") == true {
        return Self.response(request, status: 200, json: #"{"challengeId":"google-1"}"#)
      }
      return Self.response(request, status: 200, json: Self.sessionJSON)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)

    let challenge = try await client.beginAuthentication(provider: .google)
    _ = try await client.exchangeGoogleCredential(
      challengeId: challenge.challengeId,
      credential: PremiumGoogleCredential(idToken: "google-token")
    )

    let requests = await transport.recordedRequests()
    XCTAssertEqual(requests[1].url?.path, "/api/auth/google/native/exchange")
    XCTAssertEqual(
      try Self.jsonObject(requests[1]),
      ["challengeId": "google-1", "idToken": "google-token"]
    )
  }

  func testCancelAuthenticationReturnsToSignedOut() async throws {
    let client = try PremiumSessionClient(
      baseURL: baseURL,
      device: device,
      tokenStore: MemoryTokenStore(),
      transport: RecordingTransport { request in
        Self.response(request, status: 200, json: #"{"challengeId":"cancel-1"}"#)
      }
    )

    _ = try await client.beginAuthentication(provider: .apple)
    let authenticatingState = await client.state
    XCTAssertEqual(
      authenticatingState,
      .authenticating(
        PremiumAuthenticationContext(provider: .apple, challengeId: "cancel-1")
      )
    )

    await client.cancelAuthentication()

    let canceledState = await client.state
    XCTAssertEqual(canceledState, .signedOut)
  }

  func testRefreshIsSingleFlightAndAccessTokenStaysInsideClient() async throws {
    let tokenStore = MemoryTokenStore("stored-refresh")
    let counter = RequestCounter()
    let transport = RecordingTransport { request in
      await counter.increment()
      try await Task.sleep(nanoseconds: 50_000_000)
      return Self.response(request, status: 200, json: Self.sessionJSON)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)

    async let first = client.refresh(force: true)
    async let second = client.refresh(force: true)
    let accounts = try await [first, second]

    XCTAssertEqual(accounts[0], accounts[1])
    let refreshCount = await counter.value()
    let recordedRequests = await transport.recordedRequests()
    XCTAssertEqual(refreshCount, 1)
    let request = try XCTUnwrap(recordedRequests.first)
    XCTAssertEqual(request.url?.path, "/api/account/session/refresh")
    XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
    XCTAssertEqual(
      try Self.jsonObject(request),
      ["refreshToken": "stored-refresh"]
    )
  }

  func testAuthenticatedRequestRefreshesOnceAfterUnauthorized() async throws {
    let tokenStore = MemoryTokenStore("refresh-old")
    let counter = RequestCounter()
    let refreshCounter = RequestCounter()
    let transport = RecordingTransport { request in
      switch request.url?.path {
      case "/api/account/session/refresh":
        let attempt = await refreshCounter.incrementAndReturn()
        let accessToken = attempt == 1 ? "access-1" : "access-2"
        return Self.response(
          request,
          status: 200,
          json: Self.sessionJSON.replacingOccurrences(of: "access-1", with: accessToken)
        )
      case "/api/private/profile":
        let attempt = await counter.incrementAndReturn()
        if attempt == 1 {
          XCTAssertEqual(
            request.value(forHTTPHeaderField: "Authorization"), "Bearer access-1")
          return Self.response(request, status: 401, json: #"{"message":"expired"}"#)
        }
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer access-2")
        return Self.response(request, status: 200, json: #"{"value":"ok"}"#)
      default:
        return Self.response(request, status: 200, json: Self.sessionJSON)
      }
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)
    _ = try await client.refresh(force: true)

    let result: ValueResponse = try await client.send(path: "private/profile")

    XCTAssertEqual(result.value, "ok")
    let requestCount = await counter.value()
    XCTAssertEqual(requestCount, 2)
  }

  func testRevokedRefreshDeletesStoredToken() async throws {
    let tokenStore = MemoryTokenStore("revoked-refresh")
    let transport = RecordingTransport { request in
      Self.response(request, status: 401, json: #"{"message":"revoked"}"#)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)

    do {
      _ = try await client.refresh(force: true)
      XCTFail("refresh should fail")
    } catch {
      XCTAssertEqual(
        error as? PremiumSessionError,
        .server(statusCode: 401, message: "revoked")
      )
    }
    let state = await client.state
    let savedToken = await tokenStore.value()
    XCTAssertEqual(state, .revoked)
    XCTAssertNil(savedToken)
  }

  func testOfflineRestorePreservesLocalFirstSignedOutOperation() async throws {
    let emptyStore = MemoryTokenStore()
    let unusedTransport = RecordingTransport { request in
      Self.response(request, status: 500, json: "{}")
    }
    let signedOutClient = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: emptyStore, transport: unusedTransport)
    await signedOutClient.restoreSession()
    let signedOutState = await signedOutClient.state
    let unusedRequests = await unusedTransport.recordedRequests()
    XCTAssertEqual(signedOutState, .signedOut)
    XCTAssertTrue(unusedRequests.isEmpty)

    let storedToken = MemoryTokenStore("refresh")
    let offlineTransport = RecordingTransport { _ in
      throw URLError(.notConnectedToInternet)
    }
    let offlineClient = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: storedToken, transport: offlineTransport)
    await offlineClient.restoreSession()
    guard case .offline = await offlineClient.state else {
      return XCTFail("expected offline state")
    }
  }

  func testDirectRefreshWithoutStoredTokenReturnsToSignedOut() async throws {
    let tokenStore = MemoryTokenStore()
    let transport = RecordingTransport { request in
      Self.response(request, status: 500, json: "{}")
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)

    do {
      _ = try await client.refresh(force: true)
      XCTFail("refresh should require a stored token")
    } catch {
      XCTAssertEqual(error as? PremiumSessionError, .missingRefreshToken)
    }

    let state = await client.state
    XCTAssertEqual(state, .signedOut)
  }

  func testRejectsCleartextNonLoopbackServer() throws {
    XCTAssertThrowsError(
      try PremiumSessionClient(
        baseURL: URL(string: "http://premium.example.test/")!,
        device: device,
        tokenStore: MemoryTokenStore(),
        transport: RecordingTransport { request in
          Self.response(request, status: 200, json: "{}")
        }
      )
    ) { error in
      XCTAssertEqual(error as? PremiumSessionError, .invalidBaseURL)
    }
  }

  func testLogoutClearsCredentialsBeforeBestEffortServerCall() async throws {
    let tokenStore = MemoryTokenStore("refresh")
    let transport = RecordingTransport { request in
      if request.url?.path.hasSuffix("/refresh") == true {
        return Self.response(request, status: 200, json: Self.sessionJSON)
      }
      XCTAssertEqual(request.url?.path, "/api/account/session/logout")
      XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer access-1")
      throw URLError(.networkConnectionLost)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)
    _ = try await client.refresh(force: true)

    do {
      try await client.logout()
      XCTFail("logout should report the server failure")
    } catch {}

    let state = await client.state
    let savedToken = await tokenStore.value()
    XCTAssertEqual(state, .signedOut)
    XCTAssertNil(savedToken)
  }

  func testDeletionHooksAndSuccessfulDeletionRevokeSession() async throws {
    let tokenStore = MemoryTokenStore("refresh")
    let hooks = RecordingHooks()
    let transport = RecordingTransport { request in
      Self.response(request, status: 200, json: Self.sessionJSON)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL,
      device: device,
      tokenStore: tokenStore,
      transport: transport,
      lifecycleHooks: hooks
    )
    _ = try await client.refresh(force: true)

    try await client.withAccountDeletion {}

    let state = await client.state
    let savedToken = await tokenStore.value()
    let hookEvents = await hooks.events()
    XCTAssertEqual(state, .revoked)
    XCTAssertNil(savedToken)
    XCTAssertEqual(hookEvents, ["delete:begin", "delete:finish:true"])
  }

  func testProviderLinkingUsesHostOwnedBearerAndHooks() async throws {
    let tokenStore = MemoryTokenStore("refresh")
    let hooks = RecordingHooks()
    let transport = RecordingTransport { request in
      switch request.url?.path {
      case "/api/account/session/refresh":
        return Self.response(request, status: 200, json: Self.sessionJSON)
      case "/api/auth/native/challenge":
        return Self.response(request, status: 200, json: #"{"challengeId":"link-google"}"#)
      case "/api/auth/google/native/exchange":
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer access-1")
        return Self.response(request, status: 200, json: Self.sessionJSON)
      default:
        return Self.response(request, status: 404, json: #"{"message":"not found"}"#)
      }
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL,
      device: device,
      tokenStore: tokenStore,
      transport: transport,
      lifecycleHooks: hooks
    )
    _ = try await client.refresh(force: true)
    let challenge = try await client.beginAuthentication(provider: .google)

    _ = try await client.linkGoogleCredential(
      challengeId: challenge.challengeId,
      credential: PremiumGoogleCredential(idToken: "google-link-token")
    )

    let hookEvents = await hooks.events()
    XCTAssertEqual(hookEvents, ["link:google:begin", "link:google:finish:true"])
  }

  func testCancelProviderLinkRestoresExistingSignedInAccount() async throws {
    let tokenStore = MemoryTokenStore("refresh-current")
    let transport = RecordingTransport { request in
      if request.url?.path.hasSuffix("/challenge") == true {
        return Self.response(request, status: 200, json: #"{"challengeId":"link-cancel"}"#)
      }
      return Self.response(request, status: 200, json: Self.sessionJSON)
    }
    let client = try PremiumSessionClient(
      baseURL: baseURL, device: device, tokenStore: tokenStore, transport: transport)
    let account = try await client.refresh(force: true)

    _ = try await client.beginAuthentication(provider: .google)
    await client.cancelAuthentication()

    let state = await client.state
    XCTAssertEqual(state, .signedIn(account))
  }

  private struct ValueResponse: Decodable, Sendable {
    let value: String
  }

  private static let sessionJSON =
    #"{"user":{"id":"account-1","email":"person@example.test","displayName":"Test Person"},"session":{"id":"session-1","token":"access-1","refreshToken":"refresh-1","userId":"account-1","deviceId":"device-1","authMethod":"apple","accessExpiresAt":"2026-07-28T13:00:00Z"}}"#

  private static func response(
    _ request: URLRequest, status: Int, json: String
  ) -> (Data, HTTPURLResponse) {
    let payload =
      (200..<300).contains(status)
      ? #"{"ok":true,"result":\#(json)}"#
      : #"{"ok":false,"error":\#(json)}"#
    return (
      Data(payload.utf8),
      HTTPURLResponse(
        url: request.url!,
        statusCode: status,
        httpVersion: "HTTP/1.1",
        headerFields: ["Content-Type": "application/json"]
      )!
    )
  }

  private static func jsonObject(_ request: URLRequest) throws -> [String: String] {
    let data = try XCTUnwrap(request.httpBody)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: String])
    return object
  }
}

private actor MemoryTokenStore: PremiumRefreshTokenStore {
  private var token: String?

  init(_ token: String? = nil) {
    self.token = token
  }

  func read() async throws -> String? {
    token
  }

  func save(_ refreshToken: String) async throws {
    token = refreshToken
  }

  func delete() async throws {
    token = nil
  }

  func value() -> String? {
    token
  }
}

private actor RecordingTransport: PremiumHTTPTransport {
  typealias Handler = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)

  private let handler: Handler
  private var requests: [URLRequest] = []

  init(handler: @escaping Handler) {
    self.handler = handler
  }

  func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
    requests.append(request)
    return try await handler(request)
  }

  func recordedRequests() -> [URLRequest] {
    requests
  }
}

private actor RequestCounter {
  private var count = 0

  func increment() {
    count += 1
  }

  func incrementAndReturn() -> Int {
    count += 1
    return count
  }

  func value() -> Int {
    count
  }
}

private actor RecordingHooks: PremiumAccountLifecycleHooks {
  private var values: [String] = []

  func providerLinkingWillBegin(_ provider: PremiumIdentityProvider) async {
    values.append("link:\(provider.rawValue):begin")
  }

  func providerLinkingDidFinish(
    _ provider: PremiumIdentityProvider, succeeded: Bool
  ) async {
    values.append("link:\(provider.rawValue):finish:\(succeeded)")
  }

  func accountDeletionWillBegin() async {
    values.append("delete:begin")
  }

  func accountDeletionDidFinish(succeeded: Bool) async {
    values.append("delete:finish:\(succeeded)")
  }

  func events() -> [String] {
    values
  }
}
