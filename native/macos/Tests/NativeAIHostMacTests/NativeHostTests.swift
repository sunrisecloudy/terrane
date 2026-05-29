import Foundation
@testable import NativeAIHostMac
import Testing

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
}
