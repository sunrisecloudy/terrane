import Foundation
@testable import NativeAIHostMac
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

        let denied = reopened.get(BridgeRequest(
            id: "denied",
            method: "storage.get",
            params: ["key": "other-app:note", "defaultValue": NSNull()],
            context: context
        ))
        #expect(!denied.ok)
        #expect(denied.error?["code"] as? String == "permission_denied")
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
