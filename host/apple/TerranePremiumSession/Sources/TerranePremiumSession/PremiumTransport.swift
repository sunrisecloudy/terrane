import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

public protocol PremiumHTTPTransport: Sendable {
  func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

public struct PremiumURLSessionTransport: PremiumHTTPTransport, @unchecked Sendable {
  private let session: URLSession

  public init(session: URLSession = .shared) {
    self.session = session
  }

  public func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
    let (data, response) = try await session.data(for: request)
    guard let response = response as? HTTPURLResponse else {
      throw PremiumSessionError.invalidResponse
    }
    return (data, response)
  }
}

public enum PremiumHTTPMethod: String, Sendable {
  case delete = "DELETE"
  case get = "GET"
  case patch = "PATCH"
  case post = "POST"
  case put = "PUT"
}

public struct PremiumEmptyResponse: Codable, Equatable, Sendable {
  public init() {}
}
