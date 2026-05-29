import Foundation
@testable import NativeAIHostMac
import SQLite3
import Testing
import WebKit

@Suite("macOS native host")
struct NativeHostTests {
    @Test("runtime resource locator maps runtime and generated app files")
    func runtimeResourceLocatorMapsResources() throws {
        let runtimeIndexURL = URL(string: "app-runtime://runtime/index.html")!
        let runtimeIndex = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: runtimeIndexURL))
        #expect(runtimeIndex.path.hasSuffix("runtime-web/index.html"))
        #expect(FileManager.default.fileExists(atPath: runtimeIndex.path))

        let runtimeScriptURL = URL(string: "app-runtime://runtime/runtime/runtime.js")!
        let runtimeScript = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: runtimeScriptURL))
        #expect(runtimeScript.path.hasSuffix("runtime-web/runtime.js"))
        #expect(RuntimeResourceLocator.mimeType(for: runtimeScript) == "text/javascript")

        let manifestURL = URL(string: "app-runtime://runtime/webapps/examples/notes-lite/manifest.json")!
        let manifest = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: manifestURL))
        #expect(manifest.path.hasSuffix("webapps/examples/notes-lite/manifest.json"))
        #expect(RuntimeResourceLocator.mimeType(for: manifest) == "application/json")

        let escapedURL = URL(string: "app-runtime://runtime/../../docs/00_PRD.md")!
        #expect(RuntimeResourceLocator.fileURL(forRuntimeURL: escapedURL) == nil)
    }

    @Test("SQLite storage persists by app id and storage prefix")
    func sqliteStoragePersistsWithAppScope() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("native-ai-macos-storage-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let context = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["storage.read", "storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "test-mount"
        )

        do {
            let storage = PlatformStorage(databaseURL: dbURL)
            let set = storage.set(BridgeRequest(
                id: "set",
                method: "storage.set",
                params: ["key": "notes-lite:note", "value": ["title": "First note"]],
                context: context
            ))
            #expect(set.ok)
        }

        let reopened = PlatformStorage(databaseURL: dbURL)
        let get = reopened.get(BridgeRequest(
            id: "get",
            method: "storage.get",
            params: ["key": "notes-lite:note", "defaultValue": NSNull()],
            context: context
        ))
        #expect(get.ok)
        let getResult = try #require(get.result as? [String: Any])
        let value = try #require(getResult["value"] as? [String: Any])
        #expect(value["title"] as? String == "First note")
        #expect(try sqliteTableExists(dbURL: dbURL, table: "app_install_reports"))

        let denied = reopened.get(BridgeRequest(
            id: "denied",
            method: "storage.get",
            params: ["key": "other-app:note", "defaultValue": NSNull()],
            context: context
        ))
        #expect(!denied.ok)
        #expect(denied.error?["code"] as? String == "permission_denied")
    }

    @Test("SQLite app registry rolls back active version and preserves storage")
    func sqliteAppRegistryRollsBackActiveVersion() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("native-ai-macos-rollback-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        var smokeTestedInstallIds: [String] = []
        let registry = try PlatformAppRegistry(databaseURL: dbURL) { version in
            smokeTestedInstallIds.append(version.installId)
        }
        let manifestV1 = #"{"id":"notes-lite","version":"0.1.0","dataVersion":1}"#
        let manifestV2 = #"{"id":"notes-lite","version":"0.2.0","dataVersion":1}"#

        let first = try registry.installVersion(
            appId: "notes-lite",
            name: "Notes Lite",
            version: "0.1.0",
            manifestJSON: manifestV1,
            contentHash: "hash-v1",
            installId: "install-v1"
        )
        #expect(first.status == "enabled")

        let context = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["storage.read", "storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "rollback-test-mount"
        )
        let storage = PlatformStorage(databaseURL: dbURL)
        let set = storage.set(BridgeRequest(
            id: "set-storage",
            method: "storage.set",
            params: ["key": "notes-lite:notes", "value": [["title": "Keep me"]]],
            context: context
        ))
        #expect(set.ok)

        let second = try registry.installVersion(
            appId: "notes-lite",
            name: "Notes Lite",
            version: "0.2.0",
            manifestJSON: manifestV2,
            contentHash: "hash-v2",
            installId: "install-v2"
        )
        #expect(second.status == "enabled")
        #expect(try registry.activeVersion(appId: "notes-lite")?.installId == "install-v2")

        let rollback = try registry.rollback(appId: "notes-lite")
        #expect(rollback.activeInstallId == "install-v1")
        #expect(rollback.rolledBackInstallId == "install-v2")
        #expect(rollback.activeVersion == "0.1.0")
        #expect(smokeTestedInstallIds == ["install-v1"])
        #expect(try registry.activeVersion(appId: "notes-lite")?.status == "enabled")

        let reopenedStorage = PlatformStorage(databaseURL: dbURL)
        let get = reopenedStorage.get(BridgeRequest(
            id: "get-storage",
            method: "storage.get",
            params: ["key": "notes-lite:notes", "defaultValue": []],
            context: context
        ))
        #expect(get.ok)
        let result = try #require(get.result as? [String: Any])
        let notes = try #require(result["value"] as? [[String: Any]])
        let note = try #require(notes.first)
        #expect(note["title"] as? String == "Keep me")

        let events = try registry.installationEvents(appId: "notes-lite")
        let rollbackEvent = try #require(events.first(where: { $0.action == "rollback" }))
        #expect(rollbackEvent.installId == "install-v1")
        #expect(rollbackEvent.previousInstallId == "install-v2")
    }

    @Test("debug control plane writes token file, authenticates health, and audits requests")
    func debugControlPlaneAuthenticatesHealthAndAuditsRequests() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("native-ai-macos-control-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let tokenURL = tempDir.appendingPathComponent("control.token")
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")

        let controlPlane = try DevControlPlane(configuration: .init(
            port: 0,
            tokenFileURL: tokenURL,
            databaseURL: dbURL,
            tokenOverride: nil
        ))
        try controlPlane.start(waitUntilReady: true)
        defer {
            controlPlane.stop()
        }

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        #expect(token.count == 43)
        #expect(try posixPermissions(at: tokenURL) == 0o600)

        let port = try #require(controlPlane.boundPort)
        let healthURL = URL(string: "http://127.0.0.1:\(port)/health")!

        let unauthorized = try await httpRequest(healthURL)
        #expect(unauthorized.statusCode == 401)
        #expect(unauthorized.body.contains("control_auth_required"))

        let authorized = try await httpRequest(
            healthURL,
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(authorized.statusCode == 200)
        #expect(authorized.body.contains(#""platform":"macos""#))
        #expect(authorized.body.contains(#""devMode":true"#))

        #expect(try sqliteControlCommandCount(dbURL: dbURL, decision: "rejected") == 1)
        #expect(try sqliteControlCommandCount(dbURL: dbURL, decision: "accepted") == 1)
    }

    @Test("core.step returns real Zig output when a dylib is available")
    func coreStepReturnsRealZigOutput() throws {
        guard let dylibPath = ProcessInfo.processInfo.environment["NATIVE_AI_ZIG_CORE_DYLIB_FOR_TEST"],
              FileManager.default.fileExists(atPath: dylibPath)
        else {
            return
        }

        let core = ZigCoreBridge(libraryPathOverride: dylibPath)
        #expect(core.isAvailable)
        let context = AppSandboxContext(
            appId: "task-workbench",
            approvedPermissions: ["core.step"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "test-mount"
        )
        let response = core.step(BridgeRequest(
            id: "core",
            method: "core.step",
            params: ["event": ["type": "CreateTask", "payload": ["title": "macOS smoke task"]]],
            context: context
        ))
        #expect(response.ok)
        let result = try #require(response.result as? [String: Any])
        #expect(result["ok"] as? Bool == true)
        let actions = try #require(result["actions"] as? [[String: Any]])
        #expect(!actions.isEmpty)
    }

    @MainActor
    @Test("file dialogs return selected files, save output, and structured cancellations")
    func fileDialogsReturnResultsAndCancellationErrors() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("native-ai-macos-dialogs-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let inputURL = tempDir.appendingPathComponent("input.txt")
        try "Dialog input".write(to: inputURL, atomically: true, encoding: .utf8)
        let outputURL = tempDir.appendingPathComponent("output.txt")
        let context = AppSandboxContext(
            appId: "file-transformer",
            approvedPermissions: ["dialog.openFile", "dialog.saveFile"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "test-mount"
        )
        let dialogs = PlatformDialogs(
            openFileURLProvider: { inputURL },
            saveFileURLProvider: { suggestedName in
                #expect(suggestedName == "output.txt")
                return outputURL
            }
        )

        let open = dialogs.openFile(BridgeRequest(
            id: "open",
            method: "dialog.openFile",
            params: [:],
            context: context
        ))
        #expect(open.ok)
        let openResult = try #require(open.result as? [String: Any])
        let files = try #require(openResult["files"] as? [[String: Any]])
        let firstFile = try #require(files.first)
        #expect(firstFile["name"] as? String == "input.txt")
        #expect(firstFile["mime"] as? String == "text/plain")
        #expect(firstFile["text"] as? String == "Dialog input")

        let save = dialogs.saveFile(BridgeRequest(
            id: "save",
            method: "dialog.saveFile",
            params: ["suggestedName": "output.txt", "text": "Saved body"],
            context: context
        ))
        #expect(save.ok)
        #expect(try String(contentsOf: outputURL, encoding: .utf8) == "Saved body")

        let cancelled = PlatformDialogs(
            openFileURLProvider: { nil },
            saveFileURLProvider: { _ in nil }
        )
        let openCancelled = cancelled.openFile(BridgeRequest(
            id: "open-cancel",
            method: "dialog.openFile",
            params: [:],
            context: context
        ))
        #expect(!openCancelled.ok)
        #expect(openCancelled.error?["code"] as? String == "dialog_cancelled")

        let saveCancelled = cancelled.saveFile(BridgeRequest(
            id: "save-cancel",
            method: "dialog.saveFile",
            params: [:],
            context: context
        ))
        #expect(!saveCancelled.ok)
        #expect(saveCancelled.error?["code"] as? String == "dialog_cancelled")
    }

    @MainActor
    @Test("WKWebView loads runtime resources and dispatches the native bridge")
    func webViewLoadsRuntimeAndDispatchesBridge() async throws {
        let bridge = WebBridge()
        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")
        defer {
            contentController.removeScriptMessageHandler(forName: "NativeAIPlatformBridge")
        }

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(RuntimeSchemeHandler(), forURLScheme: RuntimeResourceLocator.scheme)

        let webView = WKWebView(frame: CGRect(x: 0, y: 0, width: 1000, height: 700), configuration: configuration)
        webView.load(URLRequest(url: RuntimeResourceLocator.runtimeIndexURL()))

        let status = try await waitForJavaScript(
            in: webView,
            "document.querySelector('[data-testid=\"runtime-status\"]')?.textContent || ''",
            as: String.self,
            matching: { $0 == "Ready" }
        )
        #expect(status == "Ready")

        let hasNotesButton = try await waitForJavaScript(
            in: webView,
            "Boolean(document.querySelector('[data-testid=\"open-notes-lite-button\"]'))",
            as: Bool.self,
            matching: { $0 }
        )
        #expect(hasNotesButton)

        _ = try await webView.evaluateJavaScript("document.querySelector('[data-testid=\"open-notes-lite-button\"]').click(); true")

        let activeTitle = try await waitForJavaScript(
            in: webView,
            "document.querySelector('[data-testid=\"active-app-title\"]')?.textContent || ''",
            as: String.self,
            matching: { $0 == "Notes Lite" }
        )
        #expect(activeTitle == "Notes Lite")

        let hasFrame = try await waitForJavaScript(
            in: webView,
            "Boolean(document.querySelector('[data-testid=\"runtime-app-frame\"]'))",
            as: Bool.self,
            matching: { $0 }
        )
        #expect(hasFrame)

        let bridgeLogText = try await waitForJavaScript(
            in: webView,
            "document.querySelector('[data-testid=\"bridge-log\"]')?.textContent || ''",
            as: String.self,
            matching: { $0.contains("notes-lite runtime.capabilities ok") }
        )
        #expect(bridgeLogText.contains("notes-lite runtime.capabilities ok"))
    }
}

private func sqliteTableExists(dbURL: URL, table: String) throws -> Bool {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return false
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, table, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    return sqlite3_step(statement) == SQLITE_ROW
}

private func sqliteControlCommandCount(dbURL: URL, decision: String) throws -> Int {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return 0
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM control_commands WHERE decision = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, decision, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW else {
        return 0
    }
    return Int(sqlite3_column_int(statement, 0))
}

private func posixPermissions(at url: URL) throws -> Int {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return attributes[.posixPermissions] as? Int ?? 0
}

private struct HTTPTestResponse {
    let statusCode: Int
    let body: String
}

private func httpRequest(_ url: URL, headers: [String: String] = [:]) async throws -> HTTPTestResponse {
    var request = URLRequest(url: url)
    for (name, value) in headers {
        request.setValue(value, forHTTPHeaderField: name)
    }
    let (data, response) = try await URLSession.shared.data(for: request)
    let httpResponse = try #require(response as? HTTPURLResponse)
    return HTTPTestResponse(
        statusCode: httpResponse.statusCode,
        body: String(data: data, encoding: .utf8) ?? ""
    )
}

enum NativeHostTestError: Error, CustomStringConvertible {
    case timedOut(String)

    var description: String {
        switch self {
        case let .timedOut(script):
            return "Timed out waiting for JavaScript condition: \(script)"
        }
    }
}

@MainActor
private func waitForJavaScript<T>(
    in webView: WKWebView,
    _ script: String,
    as type: T.Type,
    matching predicate: (T) -> Bool,
    timeoutSeconds: TimeInterval = 8.0
) async throws -> T {
    let deadline = Date().addingTimeInterval(timeoutSeconds)
    var latestValue: T?

    while Date() < deadline {
        do {
            if let value = try await webView.evaluateJavaScript(script) as? T {
                latestValue = value
                if predicate(value) {
                    return value
                }
            }
        } catch {
            // The page may still be navigating; keep polling until the timeout.
        }
        try await Task.sleep(nanoseconds: 100_000_000)
    }

    if let latestValue {
        return latestValue
    }
    throw NativeHostTestError.timedOut(script)
}
