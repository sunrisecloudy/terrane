import Foundation
import XCTest

final class McpLoopbackServerTests: XCTestCase {
  func testParsesPermissionRequestForTrustedGuiApproval() throws {
    let response =
      #"{"jsonrpc":"2.0","id":3,"result":{"isError":true,"structuredContent":{"type":"permission_required","app":"notes","appName":"Notes","missingResources":["kv"],"message":"permission required"}}}"#
    let prompt = try XCTUnwrap(PermissionRequiredPrompt.parse(mcpResponse: response))
    XCTAssertEqual(prompt.appId, "notes")
    XCTAssertEqual(prompt.appName, "Notes")
    XCTAssertEqual(prompt.missingResources, ["kv"])
  }

  func testPublishesAuthenticatedMcpBackedByGuiCore() throws {
    let home = FileManager.default.temporaryDirectory
      .appendingPathComponent("terrane-gui-mcp-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: home) }

    guard let bridge = TerraneBridge(home: home) else {
      return XCTFail("cannot open temporary Terrane home")
    }
    let server = McpLoopbackServer(home: home, bridge: bridge)
    try server.start()
    defer {
      server.stop()
      bridge.close()
    }

    let discoveryURL = home.appendingPathComponent(McpLoopbackServer.discoveryFileName)
    let discovery = try waitForDiscovery(at: discoveryURL)
    let endpoint = try XCTUnwrap(URL(string: try XCTUnwrap(discovery["endpoint"] as? String)))
    let token = try XCTUnwrap(discovery["token"] as? String)
    let attributes = try FileManager.default.attributesOfItem(atPath: discoveryURL.path)
    XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)

    let unauthorized = try post(
      endpoint,
      token: "wrong-token",
      body:
        #"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#
    )
    XCTAssertEqual(unauthorized.status, 401)

    let initialize = try post(
      endpoint,
      token: token,
      body:
        #"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#
    )
    XCTAssertEqual(initialize.status, 200)
    XCTAssertTrue(initialize.body.contains(#""name":"terrane-mcp""#), initialize.body)

    let apps = try post(
      endpoint,
      token: token,
      body:
        #"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#
    )
    XCTAssertEqual(apps.status, 200)
    XCTAssertTrue(apps.body.contains(#""apps":[]"#), apps.body)

    server.stop()
    XCTAssertFalse(FileManager.default.fileExists(atPath: discoveryURL.path))
  }

  private func waitForDiscovery(at url: URL) throws -> [String: Any] {
    let deadline = Date().addingTimeInterval(3)
    while Date() < deadline {
      if let data = try? Data(contentsOf: url),
        let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
      {
        return object
      }
      RunLoop.current.run(until: Date().addingTimeInterval(0.01))
    }
    throw NSError(
      domain: "McpLoopbackServerTests",
      code: 1,
      userInfo: [NSLocalizedDescriptionKey: "MCP discovery was not published"]
    )
  }

  private func post(_ url: URL, token: String, body: String) throws -> (status: Int, body: String) {
    let done = expectation(description: "POST \(url.path)")
    var capturedStatus = 0
    var capturedBody = ""
    var capturedError: Error?
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = Data(body.utf8)
    URLSession.shared.dataTask(with: request) { data, response, error in
      capturedStatus = (response as? HTTPURLResponse)?.statusCode ?? 0
      capturedBody = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
      capturedError = error
      done.fulfill()
    }.resume()
    wait(for: [done], timeout: 5)
    if let capturedError { throw capturedError }
    return (capturedStatus, capturedBody)
  }
}
