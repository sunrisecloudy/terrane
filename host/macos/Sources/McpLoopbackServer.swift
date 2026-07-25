import Foundation
import Network
import Security

/// Authenticated loopback MCP endpoint backed by the GUI's live Terrane Core.
///
/// The endpoint is published under the Terrane home so `terrane-mcp` can attach
/// when the GUI owns the home lock. The discovery file is user-readable only
/// and contains a per-launch bearer token.
final class McpLoopbackServer {
  static let discoveryFileName = "mcp-gui.json"
  private static let maximumRequestBytes = 16 * 1024 * 1024

  private struct Discovery: Codable {
    let version: Int
    let endpoint: String
    let health: String
    let token: String
    let pid: Int32
  }

  private struct Request {
    let method: String
    let path: String
    let headers: [String: String]
    let body: Data
  }

  private let home: URL
  private let bridge: TerraneBridge
  private let queue = DispatchQueue(label: "com.terrane.host.mcp-loopback")
  private let token = McpLoopbackServer.randomToken()
  private var listener: NWListener?
  private var baseURL = ""
  private var stopped = false

  private var discoveryURL: URL {
    home.appendingPathComponent(Self.discoveryFileName)
  }

  init(home: URL, bridge: TerraneBridge) {
    self.home = home
    self.bridge = bridge
  }

  func start() throws {
    let parameters = NWParameters.tcp
    parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
    let listener = try NWListener(using: parameters)
    self.listener = listener
    listener.newConnectionHandler = { [weak self] connection in
      self?.accept(connection)
    }
    listener.stateUpdateHandler = { [weak self, weak listener] state in
      guard let self else { return }
      switch state {
      case .ready:
        guard let port = listener?.port?.rawValue else { return }
        self.baseURL = "http://127.0.0.1:\(port)"
        do {
          try self.publishDiscovery()
          NSLog("terrane-host: MCP attached at \(self.baseURL)/mcp")
        } catch {
          NSLog("terrane-host: cannot publish MCP endpoint: \(error)")
          listener?.cancel()
        }
      case .failed(let error):
        NSLog("terrane-host: MCP loopback listener failed: \(error)")
        self.removeOwnedDiscovery()
      case .cancelled:
        self.removeOwnedDiscovery()
      default:
        break
      }
    }
    listener.start(queue: queue)
  }

  func stop() {
    queue.sync {
      stopped = true
      listener?.cancel()
      listener = nil
      removeOwnedDiscovery()
    }
  }

  private func accept(_ connection: NWConnection) {
    guard !stopped else {
      connection.cancel()
      return
    }
    connection.start(queue: queue)
    receive(on: connection, buffer: Data())
  }

  private func receive(on connection: NWConnection, buffer: Data) {
    connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) {
      [weak self] content, _, complete, error in
      guard let self else {
        connection.cancel()
        return
      }
      var next = buffer
      if let content {
        next.append(content)
      }
      guard next.count <= Self.maximumRequestBytes else {
        self.respond(connection, status: 413, body: #"{"error":"request too large"}"#)
        return
      }
      if let request = Self.parseRequest(next) {
        self.route(request, on: connection)
      } else if complete || error != nil {
        self.respond(connection, status: 400, body: #"{"error":"bad request"}"#)
      } else {
        self.receive(on: connection, buffer: next)
      }
    }
  }

  private func route(_ request: Request, on connection: NWConnection) {
    guard request.headers["authorization"] == "Bearer \(token)" else {
      respond(connection, status: 401, body: #"{"error":"unauthorized"}"#)
      return
    }

    if request.method == "GET", request.path == "/health" {
      respond(connection, status: 200, body: #"{"status":"ready","transport":"gui"}"#)
      return
    }

    guard request.method == "POST", request.path == "/mcp",
      let raw = String(data: request.body, encoding: .utf8)
    else {
      respond(connection, status: 404, body: #"{"error":"not found"}"#)
      return
    }

    let result = bridge.handleMcp(request: raw, adminBaseURL: baseURL)
    guard result.0 else {
      respond(connection, status: 500, body: Self.errorJSON(result.1))
      return
    }
    if let prompt = PermissionRequiredPrompt.parse(mcpResponse: result.1) {
      bridge.requestPermission(prompt) { [weak self] approved in
        guard let self else {
          connection.cancel()
          return
        }
        self.queue.async {
          guard approved else {
            self.respond(connection, status: 200, body: result.1)
            return
          }
          let retry = self.bridge.handleMcp(request: raw, adminBaseURL: self.baseURL)
          guard retry.0 else {
            self.respond(connection, status: 500, body: Self.errorJSON(retry.1))
            return
          }
          self.respond(connection, status: retry.1.isEmpty ? 202 : 200, body: retry.1)
        }
      }
      return
    }
    respond(connection, status: result.1.isEmpty ? 202 : 200, body: result.1)
  }

  private func respond(_ connection: NWConnection, status: Int, body: String) {
    let data = body.data(using: .utf8) ?? Data()
    let reason: String
    switch status {
    case 200: reason = "OK"
    case 202: reason = "Accepted"
    case 400: reason = "Bad Request"
    case 401: reason = "Unauthorized"
    case 404: reason = "Not Found"
    case 413: reason = "Payload Too Large"
    default: reason = "Internal Server Error"
    }
    let header =
      "HTTP/1.1 \(status) \(reason)\r\n"
      + "Content-Type: application/json\r\n"
      + "Content-Length: \(data.count)\r\n"
      + "Connection: close\r\n\r\n"
    var response = header.data(using: .utf8) ?? Data()
    response.append(data)
    connection.send(content: response, completion: .contentProcessed { _ in
      connection.cancel()
    })
  }

  private func publishDiscovery() throws {
    try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    let discovery = Discovery(
      version: 1,
      endpoint: "\(baseURL)/mcp",
      health: "\(baseURL)/health",
      token: token,
      pid: ProcessInfo.processInfo.processIdentifier
    )
    let data = try JSONEncoder().encode(discovery)
    try data.write(to: discoveryURL, options: .atomic)
    try FileManager.default.setAttributes(
      [.posixPermissions: NSNumber(value: Int16(0o600))],
      ofItemAtPath: discoveryURL.path
    )
  }

  private func removeOwnedDiscovery() {
    guard let data = try? Data(contentsOf: discoveryURL),
      let discovery = try? JSONDecoder().decode(Discovery.self, from: data),
      discovery.token == token
    else {
      return
    }
    try? FileManager.default.removeItem(at: discoveryURL)
  }

  private static func parseRequest(_ data: Data) -> Request? {
    let marker = Data("\r\n\r\n".utf8)
    guard let headerRange = data.range(of: marker),
      let headerText = String(data: data[..<headerRange.lowerBound], encoding: .utf8)
    else {
      return nil
    }
    let lines = headerText.components(separatedBy: "\r\n")
    guard let requestLine = lines.first else { return nil }
    let parts = requestLine.split(separator: " ")
    guard parts.count == 3 else { return nil }

    var headers: [String: String] = [:]
    for line in lines.dropFirst() {
      guard let colon = line.firstIndex(of: ":") else { continue }
      let key = line[..<colon].trimmingCharacters(in: .whitespaces).lowercased()
      let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
      headers[key] = value
    }
    guard let contentLength = Int(headers["content-length"] ?? "0"), contentLength >= 0 else {
      return nil
    }
    let bodyStart = headerRange.upperBound
    guard data.count >= bodyStart + contentLength else { return nil }
    let body = data.subdata(in: bodyStart..<(bodyStart + contentLength))
    return Request(
      method: String(parts[0]),
      path: String(parts[1]),
      headers: headers,
      body: body
    )
  }

  private static func randomToken() -> String {
    var bytes = [UInt8](repeating: 0, count: 32)
    if SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess {
      return Data(bytes).base64EncodedString()
    }
    return UUID().uuidString + UUID().uuidString
  }

  private static func errorJSON(_ message: String) -> String {
    let object = ["error": message]
    guard let data = try? JSONSerialization.data(withJSONObject: object),
      let json = String(data: data, encoding: .utf8)
    else {
      return #"{"error":"internal error"}"#
    }
    return json
  }
}
