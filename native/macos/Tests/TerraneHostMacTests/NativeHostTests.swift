import Foundation
@testable import TerraneHostMac
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
        let engineRoomScriptURL = URL(string: "app-runtime://runtime/runtime/engine-room.js")!
        let engineRoomScript = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: engineRoomScriptURL))
        #expect(engineRoomScript.path.hasSuffix("runtime-web/engine-room.js"))
        #expect(RuntimeResourceLocator.mimeType(for: engineRoomScript) == "text/javascript")
        let engineRoomStylesURL = URL(string: "app-runtime://runtime/runtime/engine-room.css")!
        let engineRoomStyles = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: engineRoomStylesURL))
        #expect(engineRoomStyles.path.hasSuffix("runtime-web/engine-room.css"))
        #expect(RuntimeResourceLocator.mimeType(for: engineRoomStyles) == "text/css")

        let manifestURL = URL(string: "app-runtime://runtime/webapps/examples/notes-lite/manifest.json")!
        let manifest = try #require(RuntimeResourceLocator.fileURL(forRuntimeURL: manifestURL))
        #expect(manifest.path.hasSuffix("webapps/examples/notes-lite/manifest.json"))
        #expect(RuntimeResourceLocator.mimeType(for: manifest) == "application/json")
        #expect(RuntimeResourceLocator.exampleManifestURL(for: "notes-lite")?.path.hasSuffix("webapps/examples/notes-lite/manifest.json") == true)
        #expect(RuntimeResourceLocator.exampleFileURL(appId: "notes-lite", path: "../manifest.json") == nil)

        let migrations = try #require(RuntimeResourceLocator.sqliteMigrationsDirectoryURL())
        #expect(migrations.path.hasSuffix("db/sqlite"))
        #expect(FileManager.default.fileExists(atPath: migrations.appendingPathComponent("001_initial.sql").path))

        let escapedURL = URL(string: "app-runtime://runtime/../../docs/00_PRD.md")!
        #expect(RuntimeResourceLocator.fileURL(forRuntimeURL: escapedURL) == nil)
    }

    @Test("runtime resource locator recognizes the native Engine Room snapshot endpoint")
    func runtimeResourceLocatorRecognizesEngineRoomSnapshotEndpoint() throws {
        let snapshotURL = URL(string: "app-runtime://runtime/engine-room/snapshot")!
        #expect(RuntimeResourceLocator.isEngineRoomSnapshotURL(snapshotURL))
        #expect(RuntimeResourceLocator.fileURL(forRuntimeURL: snapshotURL) == nil)
    }

    @Test("native Engine Room snapshot includes bundled apps and database state")
    func nativeEngineRoomSnapshotIncludesBundledAppsAndDatabaseState() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-engine-room-\(UUID().uuidString)", isDirectory: true)
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
            mountToken: "engine-room-test"
        )
        let storage = PlatformStorage(databaseURL: dbURL)
        let write = storage.set(BridgeRequest(
            id: "engine-room-storage-set",
            method: "storage.set",
            params: ["key": "notes-lite:debug", "value": ["title": "Visible at start"]],
            context: context
        ))
        #expect(write.ok)

        let provider = NativeEngineRoomSnapshotProvider(databaseURL: dbURL)
        let snapshot = provider.snapshot(appId: nil, limit: 10)
        let overview = try #require(snapshot["overview"] as? [String: Any])
        #expect(overview["source"] as? String == "macos-runtime-scheme")

        let apps = try #require(snapshot["apps"] as? [String: Any])
        let installed = try #require(apps["installed"] as? [[String: Any]])
        #expect(installed.contains { ($0["appId"] as? String) == "notes-lite" })

        let storageSection = try #require(snapshot["storage"] as? [String: Any])
        let rows = try #require(storageSection["rows"] as? [[String: Any]])
        #expect(rows.contains { ($0["app_id"] as? String) == "notes-lite" && ($0["key"] as? String) == "notes-lite:debug" })

        let database = try #require(snapshot["database"] as? [String: Any])
        let counts = try #require(database["tableCounts"] as? [String: Int])
        #expect((counts["app_storage"] ?? 0) >= 1)
    }

    @Test("native app catalog loads bundled generated apps")
    func nativeAppCatalogLoadsBundledApps() throws {
        let apps = try MacAppCatalog().loadBundledApps()
        let ids = Set(apps.map(\.id))

        #expect(ids.contains("notes-lite"))
        #expect(ids.contains("task-workbench"))
        let notesLite = try #require(apps.first(where: { $0.id == "notes-lite" }))
        #expect(notesLite.name == "Notes Lite")
        #expect(notesLite.version == "0.1.0")
        #expect(notesLite.contentRatingLabel == "4+")
        #expect(!notesLite.description.isEmpty)
    }

    @Test("native window starts at a Finder-scale content size")
    func nativeWindowStartsAtFinderScaleContentSize() throws {
        let wideScreen = NSRect(x: 0, y: 0, width: 1440, height: 900)
        let compactScreen = NSRect(x: 0, y: 0, width: 920, height: 620)

        #expect(NativeWindowConfiguration.initialContentRect(visibleFrame: wideScreen).size == NSSize(width: 1080, height: 720))
        #expect(NativeWindowConfiguration.initialContentRect(visibleFrame: compactScreen).size == NSSize(width: 860, height: 560))
        #expect(NativeWindowConfiguration.minimumContentSize == NSSize(width: 860, height: 560))
        #expect(NativeWindowConfiguration.collectionBehavior.contains(.fullScreenNone))
        #expect(
            NativeWindowConfiguration.fullScreenContentSize(
                proposedSize: NSSize(width: 1080, height: 720),
                screenFrame: NSRect(x: 0, y: 0, width: 1440, height: 900)
            ) == NSSize(width: 1440, height: 900)
        )
        #expect(
            NativeWindowConfiguration.zoomedFrame(
                defaultFrame: NSRect(x: 10, y: 10, width: 1080, height: 720),
                screenVisibleFrame: NSRect(x: 0, y: 24, width: 1440, height: 876)
            ) == NSRect(x: 0, y: 24, width: 1440, height: 876)
        )
        #expect(
            NativeWindowConfiguration.toggledZoomFrame(
                currentFrame: NSRect(x: 100, y: 100, width: 1080, height: 720),
                screenVisibleFrame: NSRect(x: 0, y: 24, width: 1440, height: 876)
            ) == NSRect(x: 0, y: 24, width: 1440, height: 876)
        )
    }

    @MainActor
    @Test("native runtime script can open marketplace without mounting an app")
    func nativeRuntimeScriptCanOpenMarketplaceWithoutMountingApp() throws {
        let script = WebHostView.nativeRuntimeUpdateScript(
            appId: nil,
            showMarketplace: true,
            nativeHostModeEnabled: true
        )

        #expect(script.contains("host.setHostMode(true);"))
        #expect(script.contains("await host.showMarketplace();"))
        #expect(!script.contains("host.mountApp"))
    }

    @MainActor
    @Test("native runtime script clears marketplace mode before mounting an app")
    func nativeRuntimeScriptClearsMarketplaceModeBeforeMountingApp() throws {
        let script = WebHostView.nativeRuntimeUpdateScript(
            appId: "premium-todo",
            nativeHostModeEnabled: true
        )

        #expect(script.contains("host.setHostMode(true);"))
        #expect(script.contains(#"await host.mountApp("premium-todo");"#))
        #expect(!script.contains("host.showMarketplace"))
        #expect(!script.contains("host.showEngineRoom"))
    }

    @Test("Engine Room preference is visible by default and persists when hidden")
    func engineRoomPreferencePersistsVisibility() throws {
        let defaults = UserDefaults.standard
        let previous = defaults.object(forKey: NativeShellPreferences.engineRoomVisibleKey)
        defer {
            if let previous {
                defaults.set(previous, forKey: NativeShellPreferences.engineRoomVisibleKey)
            } else {
                defaults.removeObject(forKey: NativeShellPreferences.engineRoomVisibleKey)
            }
        }

        defaults.removeObject(forKey: NativeShellPreferences.engineRoomVisibleKey)
        #expect(NativeShellPreferences.isEngineRoomVisible == true)

        NativeShellPreferences.setEngineRoomVisible(false)
        #expect(NativeShellPreferences.isEngineRoomVisible == false)

        NativeShellPreferences.setEngineRoomVisible(true)
        #expect(NativeShellPreferences.isEngineRoomVisible == true)
    }

    @MainActor
    @Test("native runtime script can open Engine Room without mounting an app")
    func nativeRuntimeScriptCanOpenEngineRoomWithoutMountingApp() throws {
        let script = WebHostView.nativeRuntimeUpdateScript(
            appId: nil,
            showEngineRoom: true,
            nativeHostModeEnabled: true
        )

        #expect(script.contains("host.setHostMode(true);"))
        #expect(script.contains("await host.showEngineRoom();"))
        #expect(!script.contains("host.mountApp"))
        #expect(!script.contains("host.showMarketplace"))
    }

    @Test("runtime crash recovery records a failed session and reload offer")
    func runtimeCrashRecoveryRecordsFailedSessionAndReloadOffer() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-crash-recovery-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let recovery = RuntimeCrashRecovery(databaseURL: dbURL)
        let sessionId = RuntimeCrashRecovery.newSessionId()

        recovery.startRuntimeSession(
            sessionId: sessionId,
            activeAppId: "notes-lite",
            activeInstallId: "install-notes-lite"
        )
        let crash = recovery.recordWebContentProcessTerminated(
            sessionId: sessionId,
            previousMountCompletedReady: false
        )

        #expect(crash.sessionId == sessionId)
        #expect(crash.reloadOffered)
        #expect(!crash.canAutoRemount)
        let storedSession = try sqliteRuntimeSession(dbURL: dbURL, sessionId: sessionId)
        let row = try #require(storedSession)
        #expect(row.status == "failed")
        #expect(row.activeAppId == "notes-lite")
        #expect(row.activeInstallId == "install-notes-lite")
        #expect(row.endedAt != nil)
        #expect(row.metadata.contains(#""reason":"web_content_process_terminated""#))
        #expect(row.metadata.contains(#""reloadOffered":true"#))
        #expect(row.metadata.contains(#""canAutoRemount":false"#))
    }

    @Test("production guard rejects exact dev-only startup flags")
    func productionGuardRejectsExactDevOnlyStartupFlags() throws {
        #expect(NativeProductionGuard.rejectedDevOnlyFlag(
            in: ["TerraneHostMac", "--control-plane-port"],
            allowDevFlags: false
        ) == "--control-plane-port")
        #expect(NativeProductionGuard.rejectedDevOnlyFlag(
            in: ["TerraneHostMac", "--allow-runtime-mismatch=true"],
            allowDevFlags: false
        ) == "--allow-runtime-mismatch=true")
        #expect(NativeProductionGuard.rejectedDevOnlyFlag(
            in: ["TerraneHostMac", "--allow-unsigned-dev"],
            allowDevFlags: false
        ) == "--allow-unsigned-dev")
        #expect(NativeProductionGuard.rejectedDevOnlyFlag(
            in: ["TerraneHostMac", "--control-plane-portish"],
            allowDevFlags: false
        ) == nil)
        #expect(NativeProductionGuard.rejectedDevOnlyFlag(
            in: ["TerraneHostMac", "--control-plane-port"],
            allowDevFlags: true
        ) == nil)

        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-production-guard-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let rejected = NativeProductionGuard.rejectDevOnlyFlagsIfNeeded(
            arguments: ["TerraneHostMac", "--allow-unsigned-dev"],
            allowDevFlags: false,
            databaseURL: dbURL
        )
        #expect(rejected)
        #expect(try sqliteControlCommandCount(dbURL: dbURL, decision: "rejected") == 1)
    }

    @Test("SQLite storage persists by app id and storage prefix")
    func sqliteStoragePersistsWithAppScope() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-storage-\(UUID().uuidString)", isDirectory: true)
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

    @Test("SQLite storage enforces manifest maxStorageBytes")
    func sqliteStorageEnforcesManifestMaxStorageBytes() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-storage-budget-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let context = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            resourceBudget: ["maxStorageBytes": 8],
            mountToken: "storage-budget-test-mount"
        )

        let storage = PlatformStorage(databaseURL: dbURL)
        let response = storage.set(BridgeRequest(
            id: "set-too-large",
            method: "storage.set",
            params: ["key": "notes-lite:note", "value": ["title": "this is too large"]],
            context: context
        ))
        #expect(!response.ok)
        #expect(response.error?["code"] as? String == "resource_budget_exceeded")
        let details = try #require(response.error?["details"] as? [String: Any])
        #expect(details["budget"] as? String == "maxStorageBytes")
        #expect(jsonInt(details["current"]) > 8)
        #expect(jsonInt(details["max"]) == 8)
        #expect(jsonInt(details["limit"]) == 8)
    }

    @Test("SQLite storage returns storage_error when the database cannot open")
    func sqliteStorageReturnsStorageErrorWhenDatabaseCannotOpen() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-storage-error-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let context = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["storage.read", "storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "storage-error-test-mount"
        )

        let response = PlatformStorage(databaseURL: tempDir).set(BridgeRequest(
            id: "storage-error",
            method: "storage.set",
            params: ["key": "notes-lite:note", "value": ["title": "Cannot write"]],
            context: context
        ))

        #expect(!response.ok)
        #expect(response.error?["code"] as? String == "storage_error")
        #expect(response.error?["message"] as? String == "storage.set failed")
        let details = try #require(response.error?["details"] as? [String: Any])
        #expect(details["operation"] as? String == "storage.set")
        #expect(details["appId"] as? String == "notes-lite")
    }

    @Test("network.request policy denials include bridge fixture detail subsets")
    func networkRequestPolicyDenialsIncludeBridgeFixtureDetailSubsets() throws {
        let fixtureNames = [
            "invalid-network-credential-header.json",
            "invalid-network-header-denied.json",
            "invalid-network-private-denied.json",
            "invalid-network-body-too-large.json",
            "invalid-network-path-prefix-denied.json",
            "valid-network-policy-denied.json",
        ]
        let network = PlatformNetwork()

        for fixtureName in fixtureNames {
            let fixture = try bridgeFixture(fixtureName)
            let context = try bridgeFixtureContext(fixture)
            let response = network.request(BridgeRequest(
                id: fixture["id"] as? String,
                method: try #require(fixture["method"] as? String),
                params: try #require(fixture["params"] as? [String: Any]),
                context: context
            ))

            try expectBridgeDictionary(response.asDictionary(), matches: fixture)
        }
    }

    @Test("SQLite app registry rolls back active version and preserves storage")
    func sqliteAppRegistryRollsBackActiveVersion() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-rollback-\(UUID().uuidString)", isDirectory: true)
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

    @Test("debug control plane persists its signing key in Keychain")
    func debugControlPlanePersistsSigningKeyInKeychain() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-signing-key-\(UUID().uuidString)", isDirectory: true)
        let signingKeyAccount = "terrane-macos-signing-key-\(UUID().uuidString)"
        DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
            DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        }

        let first = try DevControlPlane(configuration: .init(
            port: 0,
            tokenFileURL: tempDir.appendingPathComponent("first.token"),
            databaseURL: tempDir.appendingPathComponent("first.sqlite"),
            tokenOverride: "first-token",
            signingKeyAccount: signingKeyAccount
        ))
        let firstKeyId = first.platformSigningKeyId
        first.stop()

        let second = try DevControlPlane(configuration: .init(
            port: 0,
            tokenFileURL: tempDir.appendingPathComponent("second.token"),
            databaseURL: tempDir.appendingPathComponent("second.sqlite"),
            tokenOverride: "second-token",
            signingKeyAccount: signingKeyAccount
        ))
        defer {
            second.stop()
        }

        #expect(second.platformSigningKeyId == firstKeyId)
        #expect(second.platformSigningKeyId.hasPrefix("platform-host:macos:"))
    }

    @Test("debug control plane rejects tampered installed packages before open")
    func debugControlPlaneRejectsTamperedInstalledPackageBeforeOpen() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-verified-mount-\(UUID().uuidString)", isDirectory: true)
        let signingKeyAccount = "terrane-macos-verified-mount-\(UUID().uuidString)"
        DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
            DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        }

        let tokenURL = tempDir.appendingPathComponent("control.token")
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let controlPlane = try DevControlPlane(configuration: .init(
            port: 0,
            tokenFileURL: tokenURL,
            databaseURL: dbURL,
            tokenOverride: nil,
            signingKeyAccount: signingKeyAccount
        ))
        try controlPlane.start(waitUntilReady: true)
        defer {
            controlPlane.stop()
        }

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        let commandURL = URL(string: "http://127.0.0.1:\(try #require(controlPlane.boundPort))/control/command")!
        let repoRoot = RuntimeResourceLocator.repoRootURL()
        let install = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString([
                "tool": "platform.install_webapp_package",
                "args": ["path": repoRoot.appendingPathComponent("webapps/examples/notes-lite").path],
            ])
        )
        #expect(install.statusCode == 200)
        let installResult = try jsonResult(install)
        let installId = try #require(installResult["installId"] as? String)

        let openBeforeTamper = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.open_webapp","args":{"appId":"notes-lite"}}"#
        )
        #expect(openBeforeTamper.statusCode == 200)

        try sqliteAppendToAppFile(
            dbURL: dbURL,
            installId: installId,
            path: "app.js",
            suffix: "\n// tampered after signing"
        )

        let openAfterTamper = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.open_webapp","args":{"appId":"notes-lite"}}"#
        )
        #expect(openAfterTamper.statusCode == 400)
        #expect(openAfterTamper.body.contains("content_tampered"))
    }

    @Test("debug control plane writes token file, authenticates health, and audits requests")
    func debugControlPlaneAuthenticatesHealthAndAuditsRequests() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-control-\(UUID().uuidString)", isDirectory: true)
        let signingKeyAccount = "terrane-macos-control-\(UUID().uuidString)"
        DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
            DevControlPlane.deleteSigningKeyForTests(account: signingKeyAccount)
        }
        let tokenURL = tempDir.appendingPathComponent("control.token")
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")

        let controlPlane = try DevControlPlane(configuration: .init(
            port: 0,
            tokenFileURL: tokenURL,
            databaseURL: dbURL,
            tokenOverride: nil,
            signingKeyAccount: signingKeyAccount
        ))
        try controlPlane.start(waitUntilReady: true)
        defer {
            controlPlane.stop()
        }

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        #expect(token.count == 43)
        #expect(try posixPermissions(at: tokenURL) == 0o600)

        let registry = try PlatformAppRegistry(databaseURL: dbURL)
        try registry.installVersion(
            appId: "notes-lite",
            name: "Notes Lite",
            version: "0.1.0",
            manifestJSON: #"{"id":"notes-lite","version":"0.1.0","dataVersion":1}"#,
            contentHash: "control-hash",
            installId: "install-control"
        )
        try registry.installVersion(
            appId: "notes-lite",
            name: "Notes Lite",
            version: "0.2.0",
            manifestJSON: #"{"id":"notes-lite","version":"0.2.0","dataVersion":1}"#,
            contentHash: "control-hash-v2",
            installId: "install-control-v2"
        )
        try registry.installVersion(
            appId: "task-workbench",
            name: "Task Workbench",
            version: "0.1.0",
            manifestJSON: #"{"id":"task-workbench","version":"0.1.0","dataVersion":1}"#,
            contentHash: "task-control-hash",
            installId: "install-task-control"
        )
        try registry.installVersion(
            appId: "api-dashboard",
            name: "API Dashboard",
            version: "0.1.0",
            manifestJSON: #"{"id":"api-dashboard","version":"0.1.0","dataVersion":1}"#,
            contentHash: "api-control-hash",
            installId: "install-api-control"
        )
        try registry.installVersion(
            appId: "file-transformer",
            name: "File Transformer",
            version: "0.1.0",
            manifestJSON: #"{"id":"file-transformer","version":"0.1.0","dataVersion":1}"#,
            contentHash: "file-control-hash",
            installId: "install-file-control"
        )
        try registry.installVersion(
            appId: "core-replay-lab",
            name: "Core Replay Lab",
            version: "0.1.0",
            manifestJSON: #"{"id":"core-replay-lab","version":"0.1.0","dataVersion":1}"#,
            contentHash: "core-replay-control-hash",
            installId: "install-core-replay-control"
        )
        let storageContext = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["storage.read", "storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "control-test-mount"
        )
        let storageSet = PlatformStorage(databaseURL: dbURL).set(BridgeRequest(
            id: "control-seed",
            method: "storage.set",
            params: ["key": "notes-lite:control-note", "value": ["title": "Visible to db.query_app_storage"]],
            context: storageContext
        ))
        #expect(storageSet.ok)
        let uninstallStorageContext = AppSandboxContext(
            appId: "core-replay-lab",
            approvedPermissions: ["storage.read", "storage.write"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "uninstall-test-mount"
        )
        let uninstallStorageSet = PlatformStorage(databaseURL: dbURL).set(BridgeRequest(
            id: "control-uninstall-seed",
            method: "storage.set",
            params: ["key": "core-replay-lab:state", "value": ["title": "Delete me on uninstall"]],
            context: uninstallStorageContext
        ))
        #expect(uninstallStorageSet.ok)

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
        #expect(authorized.body.contains(#""signingPublicKey":{"#))
        #expect(authorized.body.contains(#""storage":"keychain""#))

        let sessionURL = URL(string: "http://127.0.0.1:\(port)/control/sessions")!
        let session = try await httpRequest(
            sessionURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(session.statusCode == 200)
        #expect(session.body.contains(controlPlane.controlSessionId))

        let commandURL = URL(string: "http://127.0.0.1:\(port)/control/command")!
        let repoRoot = RuntimeResourceLocator.repoRootURL()
        let signedPackage = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString([
                "tool": "platform.sign_webapp_package",
                "args": ["path": repoRoot.appendingPathComponent("webapps/examples/notes-lite").path],
            ])
        )
        #expect(signedPackage.statusCode == 200)
        let signedPackageResult = try jsonResult(signedPackage)
        let signature = try #require(signedPackageResult["signature"] as? [String: Any])
        #expect(signature["algorithm"] as? String == "ed25519")
        #expect((signature["keyId"] as? String)?.hasPrefix("platform-host:macos:") == true)
        #expect(signature["permissionsHash"] as? String != nil)
        #expect(signature["policyHash"] as? String != nil)
        let signatureText = try #require(signature["signature"] as? String)
        #expect(!signatureText.isEmpty)

        let snapshotURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/snapshot")!
        let snapshot = try await httpRequest(snapshotURL, headers: ["X-Platform-Control-Token": token])
        #expect(snapshot.statusCode == 200)
        #expect(snapshot.body.contains(#""runtimeAttached":false"#))

        let createSnapshotURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/snapshots")!
        let createdAppSnapshot = try await httpRequest(
            createSnapshotURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"appId":"notes-lite","type":"manual"}"#
        )
        #expect(createdAppSnapshot.statusCode == 200)
        let createdAppSnapshotResult = try jsonResult(createdAppSnapshot)
        let appSnapshotId = try #require(createdAppSnapshotResult["snapshotId"] as? String)
        let appSnapshotStorage = try #require(createdAppSnapshotResult["storage"] as? [[String: Any]])
        #expect(appSnapshotStorage.contains { row in
            (row["key"] as? String) == "notes-lite:control-note"
                && ((row["value_json"] as? String)?.contains("Visible to db.query_app_storage") ?? false)
        })

        let changedStorage = PlatformStorage(databaseURL: dbURL).set(BridgeRequest(
            id: "control-change-after-snapshot",
            method: "storage.set",
            params: ["key": "notes-lite:control-note", "value": ["title": "Changed after snapshot"]],
            context: storageContext
        ))
        #expect(changedStorage.ok)

        let readAppSnapshot = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/snapshots/\(appSnapshotId)")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"action":"read"}"#
        )
        #expect(readAppSnapshot.statusCode == 200)
        let readAppSnapshotResult = try jsonResult(readAppSnapshot)
        #expect(readAppSnapshotResult["snapshotId"] as? String == appSnapshotId)
        let readSnapshotBody = try #require(readAppSnapshotResult["snapshot"] as? [String: Any])
        let readSnapshotStorage = try #require(readSnapshotBody["storage"] as? [[String: Any]])
        #expect(readSnapshotStorage.contains { row in
            (row["key"] as? String) == "notes-lite:control-note"
                && ((row["value_json"] as? String)?.contains("Visible to db.query_app_storage") ?? false)
        })

        let restoredSnapshot = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.restore_snapshot","args":{"snapshotId":"\#(appSnapshotId)"}}"#
        )
        #expect(restoredSnapshot.statusCode == 200)
        #expect(restoredSnapshot.body.contains(#""restoredStorageKeys":1"#))

        let restoredStorage = PlatformStorage(databaseURL: dbURL).get(BridgeRequest(
            id: "control-read-restored-snapshot",
            method: "storage.get",
            params: ["key": "notes-lite:control-note", "defaultValue": NSNull()],
            context: storageContext
        ))
        #expect(restoredStorage.ok)
        let restoredStorageResult = try #require(restoredStorage.result as? [String: Any])
        let restoredStorageValue = try #require(restoredStorageResult["value"] as? [String: Any])
        #expect(restoredStorageValue["title"] as? String == "Visible to db.query_app_storage")

        let createdSnapshotCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.create_snapshot","args":{"appId":"notes-lite","type":"manual"}}"#
        )
        #expect(createdSnapshotCommand.statusCode == 200)
        #expect(createdSnapshotCommand.body.contains(#""snapshotId":"snapshot_"#))
        let createdSnapshotCommandResult = try jsonResult(createdSnapshotCommand)
        let matchingSnapshotId = try #require(createdSnapshotCommandResult["snapshotId"] as? String)

        let matchingSnapshotCompare = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.compare_snapshot","args":{"leftSnapshotId":"\#(appSnapshotId)","rightSnapshotId":"\#(matchingSnapshotId)"}}"#
        )
        #expect(matchingSnapshotCompare.statusCode == 200)
        #expect(matchingSnapshotCompare.body.contains(#""equal":true"#))

        let changedBeforeRouteRestore = PlatformStorage(databaseURL: dbURL).set(BridgeRequest(
            id: "control-change-before-route-restore",
            method: "storage.set",
            params: ["key": "notes-lite:control-note", "value": ["title": "Changed before route restore"]],
            context: storageContext
        ))
        #expect(changedBeforeRouteRestore.ok)

        let changedSnapshotCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.create_snapshot","args":{"appId":"notes-lite","type":"manual"}}"#
        )
        #expect(changedSnapshotCommand.statusCode == 200)
        let changedSnapshotCommandResult = try jsonResult(changedSnapshotCommand)
        let changedSnapshotId = try #require(changedSnapshotCommandResult["snapshotId"] as? String)

        let changedSnapshotCompare = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.compare_snapshot","args":{"leftSnapshotId":"\#(appSnapshotId)","rightSnapshotId":"\#(changedSnapshotId)"}}"#
        )
        #expect(changedSnapshotCompare.statusCode == 200)
        #expect(changedSnapshotCompare.body.contains(#""equal":false"#))

        let routeRestoredSnapshot = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/snapshots/\(appSnapshotId)")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"action":"restore"}"#
        )
        #expect(routeRestoredSnapshot.statusCode == 200)

        let routeRestoredStorage = PlatformStorage(databaseURL: dbURL).get(BridgeRequest(
            id: "control-read-route-restored-snapshot",
            method: "storage.get",
            params: ["key": "notes-lite:control-note", "defaultValue": NSNull()],
            context: storageContext
        ))
        #expect(routeRestoredStorage.ok)
        let routeRestoredStorageResult = try #require(routeRestoredStorage.result as? [String: Any])
        let routeRestoredStorageValue = try #require(routeRestoredStorageResult["value"] as? [String: Any])
        #expect(routeRestoredStorageValue["title"] as? String == "Visible to db.query_app_storage")

        let command = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.health","args":{}}"#
        )
        #expect(command.statusCode == 200)
        #expect(command.body.contains(#""target":"macos""#))

        let targets = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.list_targets","args":{}}"#
        )
        #expect(targets.statusCode == 200)
        #expect(targets.body.contains(#""platform":"macos""#))

        let launchCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.launch","args":{"target":"macos"}}"#
        )
        #expect(launchCommand.statusCode == 200)
        #expect(launchCommand.body.contains(controlPlane.controlSessionId))

        let openWebapp = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.open_webapp","args":{"appId":"notes-lite"}}"#
        )
        #expect(openWebapp.statusCode == 200)
        #expect(openWebapp.body.contains(#""appId":"notes-lite""#))
        #expect(openWebapp.body.contains(#""runtimeSessionId":"runtime_"#))

        let attachedSnapshot = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.snapshot","args":{}}"#
        )
        #expect(attachedSnapshot.statusCode == 200)
        #expect(attachedSnapshot.body.contains(#""activeAppId":"notes-lite""#))
        #expect(attachedSnapshot.body.contains(#""runtimeAttached":true"#))

        let reloadRuntime = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.reload_runtime","args":{"target":"macos"}}"#
        )
        #expect(reloadRuntime.statusCode == 200)
        #expect(reloadRuntime.body.contains(#""status":"reloaded""#))

        let eventsURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/events")!
        let events = try await httpRequest(eventsURL, headers: ["X-Platform-Control-Token": token])
        #expect(events.statusCode == 200)
        #expect(events.body.contains(#""bridgeCalls":[]"#))
        #expect(events.body.contains(#""controlCommands":["#))

        let capabilitiesURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/capabilities")!
        let capabilities = try await httpRequest(capabilitiesURL, headers: ["X-Platform-Control-Token": token])
        #expect(capabilities.statusCode == 200)
        #expect(capabilities.body.contains(#""runtimeVersion":"0.1.0""#))
        #expect(capabilities.body.contains(#""runtime.capabilities":true"#))

        let capabilitiesCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.capabilities","args":{"appId":"notes-lite"}}"#
        )
        #expect(capabilitiesCommand.statusCode == 200)
        #expect(capabilitiesCommand.body.contains(#""appId":"notes-lite""#))
        #expect(capabilitiesCommand.body.contains(#""storage.get":true"#))

        let resourceUsageURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/resource-usage")!
        let resourceUsage = try await httpRequest(resourceUsageURL, headers: ["X-Platform-Control-Token": token])
        #expect(resourceUsage.statusCode == 200)
        let resourceUsageResult = try jsonResult(resourceUsage)
        #expect(jsonInt(resourceUsageResult["storageBytes"]) > 0)
        #expect(jsonInt(resourceUsageResult["bridgeCalls"]) == 0)

        let appResourceUsage = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.resource_usage","args":{"appId":"notes-lite"}}"#
        )
        #expect(appResourceUsage.statusCode == 200)
        let appResourceUsageResult = try jsonResult(appResourceUsage)
        #expect(appResourceUsageResult["appId"] as? String == "notes-lite")
        #expect(jsonInt(appResourceUsageResult["storageBytes"]) > 0)

        let accessibilityURL = URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)/accessibility")!
        let accessibility = try await httpRequest(accessibilityURL, headers: ["X-Platform-Control-Token": token])
        #expect(accessibility.statusCode == 200)
        #expect(accessibility.body.contains(#""status":"pass""#))
        #expect(accessibility.body.contains(#""document_title""#))

        let accessibilitySnapshot = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.accessibility_snapshot","args":{"appId":"notes-lite"}}"#
        )
        #expect(accessibilitySnapshot.statusCode == 200)
        #expect(accessibilitySnapshot.body.contains(#""title":"Notes Lite""#))
        #expect(accessibilitySnapshot.body.contains(#""testId":"new-note-button""#))

        let accessibilityAudit = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.run_accessibility_audit","args":{"appId":"notes-lite"}}"#
        )
        #expect(accessibilityAudit.statusCode == 200)
        #expect(accessibilityAudit.body.contains(#""no_unlabeled_controls""#))

        let accessibilityAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_accessibility","args":{"appId":"notes-lite","rule":"no_unlabeled_controls"}}"#
        )
        #expect(accessibilityAssert.statusCode == 200)
        #expect(accessibilityAssert.body.contains(#""rule":"no_unlabeled_controls""#))

        let runtimeQuery = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.query","args":{"appId":"notes-lite","testId":"new-note-button"}}"#
        )
        #expect(runtimeQuery.statusCode == 200)
        #expect(runtimeQuery.body.contains(#""kind":"testId""#))

        let runtimeScreenshot = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.screenshot","args":{"appId":"notes-lite","label":"control-smoke"}}"#
        )
        #expect(runtimeScreenshot.statusCode == 200)
        #expect(runtimeScreenshot.body.contains(#""format":"static-html-summary""#))
        #expect(runtimeScreenshot.body.contains(#""new-note-button""#))

        let clickTarget = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.click","args":{"appId":"notes-lite","testId":"new-note-button"}}"#
        )
        #expect(clickTarget.statusCode == 200)
        #expect(clickTarget.body.contains(#""tool":"runtime.click""#))

        let typeTarget = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.type","args":{"appId":"notes-lite","testId":"note-title-input","text":"Hello"}} "#
        )
        #expect(typeTarget.statusCode == 200)
        #expect(typeTarget.body.contains(#""value":"Hello""#))

        let setValueTarget = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.set_value","args":{"appId":"notes-lite","testId":"notes-search-input","value":"bridge"}}"#
        )
        #expect(setValueTarget.statusCode == 200)
        #expect(setValueTarget.body.contains(#""tool":"runtime.set_value""#))

        let pressKey = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.press_key","args":{"key":"Enter"}}"#
        )
        #expect(pressKey.statusCode == 200)
        #expect(pressKey.body.contains(#""key":"Enter""#))

        let dragTarget = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.drag","args":{"appId":"notes-lite","selector":"main"}}"#
        )
        #expect(dragTarget.statusCode == 200)
        #expect(dragTarget.body.contains(#""tool":"runtime.drag""#))

        let visibleAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: ##"{"tool":"runtime.assert_visible","args":{"appId":"notes-lite","selector":"#new-note"}}"##
        )
        #expect(visibleAssert.statusCode == 200)
        #expect(visibleAssert.body.contains(#""target":{"#))

        let textAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_text","args":{"appId":"notes-lite","text":"Notes Lite"}}"#
        )
        #expect(textAssert.statusCode == 200)
        #expect(textAssert.body.contains(#""text":"Notes Lite""#))

        let waitForText = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.wait_for","args":{"appId":"notes-lite","kind":"text","text":"Notes Lite"}}"#
        )
        #expect(waitForText.statusCode == 200)
        #expect(waitForText.body.contains(#""kind":"text""#))

        let timerAdvance = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.timer_advance","args":{"ms":250}}"#
        )
        #expect(timerAdvance.statusCode == 200)
        #expect(timerAdvance.body.contains(#""advancedMs":250"#))

        let dbSnapshotURL = URL(string: "http://127.0.0.1:\(port)/control/db/snapshot")!
        let dbSnapshot = try await httpRequest(
            dbSnapshotURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(dbSnapshot.statusCode == 200)
        #expect(dbSnapshot.body.contains(#""control_sessions":["#))
        #expect(dbSnapshot.body.contains(#""control_commands":["#))
        #expect(dbSnapshot.body.contains(controlPlane.controlSessionId))

        let dbSnapshotCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"db.snapshot","args":{}}"#
        )
        #expect(dbSnapshotCommand.statusCode == 200)
        #expect(dbSnapshotCommand.body.contains(#""app_storage":["#))

        let storageQuery = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/db/app-storage")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"appId":"notes-lite"}"#
        )
        #expect(storageQuery.statusCode == 200)
        #expect(storageQuery.body.contains("notes-lite:control-note"))

        let appVersionsQuery = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"db.query_app_versions","args":{"appId":"notes-lite"}}"#
        )
        #expect(appVersionsQuery.statusCode == 200)
        #expect(appVersionsQuery.body.contains("install-control"))

        let listWebapps = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.list_webapps","args":{}}"#
        )
        #expect(listWebapps.statusCode == 200)
        #expect(listWebapps.body.contains(#""activeInstallId":"install-control-v2""#))

        let versionsRoute = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/apps/notes-lite/versions")!,
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(versionsRoute.statusCode == 200)
        #expect(versionsRoute.body.contains(#""installId":"install-control-v2""#))

        let installReportRoute = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/apps/notes-lite/install-report")!,
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(installReportRoute.statusCode == 200)
        #expect(installReportRoute.body.contains(#""report":null"#))

        let rollbackCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.rollback_webapp","args":{"appId":"notes-lite"}}"#
        )
        #expect(rollbackCommand.statusCode == 200)
        #expect(rollbackCommand.body.contains(#""activeInstallId":"install-control""#))
        #expect(rollbackCommand.body.contains(#""rolledBackInstallId":"install-control-v2""#))

        let approvalPackageURL = repoRoot
            .appendingPathComponent("native/macos/.build/approval-update-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(
            at: approvalPackageURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try FileManager.default.copyItem(
            at: repoRoot.appendingPathComponent("webapps/examples/notes-lite"),
            to: approvalPackageURL
        )
        defer {
            try? FileManager.default.removeItem(at: approvalPackageURL)
        }
        let approvalManifestURL = approvalPackageURL.appendingPathComponent("manifest.json")
        let approvalManifestData = try Data(contentsOf: approvalManifestURL)
        var approvalManifest = try #require(try JSONSerialization.jsonObject(with: approvalManifestData) as? [String: Any])
        approvalManifest["version"] = "0.3.0"
        var approvalPermissions = approvalManifest["permissions"] as? [String] ?? []
        if !approvalPermissions.contains("network.request") {
            approvalPermissions.append("network.request")
        }
        approvalManifest["permissions"] = approvalPermissions
        var approvalCapabilities = approvalManifest["capabilities"] as? [String: Any] ?? [:]
        var optionalCapabilities = approvalCapabilities["optional"] as? [String] ?? []
        if !optionalCapabilities.contains("network.request") {
            optionalCapabilities.append("network.request")
        }
        approvalCapabilities["optional"] = optionalCapabilities
        approvalManifest["capabilities"] = approvalCapabilities
        approvalManifest["networkPolicy"] = [
            "allow": [
                [
                    "origin": "https://api.example.test",
                    "methods": ["GET"],
                    "pathPrefix": "/status",
                ],
            ],
            "denyPrivateNetwork": true,
        ]
        try jsonObjectString(approvalManifest).write(to: approvalManifestURL, atomically: true, encoding: .utf8)

        let pendingInstall = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.install_webapp_package", "args": ["path": approvalPackageURL.path]])
        )
        #expect(pendingInstall.statusCode == 200)
        let pendingInstallResult = try jsonResult(pendingInstall)
        let pendingInstallId = try #require(pendingInstallResult["installId"] as? String)
        #expect(pendingInstallResult["status"] as? String == "requires-approval")
        let pendingApproval = try #require(pendingInstallResult["approval"] as? [String: Any])
        let approvalReasons = try #require(pendingApproval["reasons"] as? [String])
        #expect(approvalReasons.contains("permissions"))
        #expect(approvalReasons.contains("networkPolicy"))
        #expect(approvalReasons.contains("capabilities"))

        let pendingList = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.list_webapps","args":{}}"#
        )
        #expect(pendingList.statusCode == 200)
        #expect(pendingList.body.contains(#""activeInstallId":"install-control""#))

        let pendingReport = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.install_report", "args": ["appId": "notes-lite", "installId": pendingInstallId]])
        )
        #expect(pendingReport.statusCode == 200)
        #expect(pendingReport.body.contains(#""status":"requires-approval""#))
        #expect(pendingReport.body.contains(#""requiresUserApproval":true"#))
        let pendingReportResult = try jsonResult(pendingReport)
        let pendingReportBody = try #require(pendingReportResult["report"] as? [String: Any])
        let pendingSecurity = try #require(pendingReportBody["security"] as? [String: Any])
        let pendingSignature = try #require(pendingSecurity["signature"] as? [String: Any])
        #expect(pendingSignature["algorithm"] as? String == "ed25519")
        #expect((pendingSignature["signature"] as? String)?.isEmpty == false)

        let approveUpdate = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.approve_webapp_update", "args": ["appId": "notes-lite", "installId": pendingInstallId]])
        )
        #expect(approveUpdate.statusCode == 200)
        #expect(approveUpdate.body.contains(#""status":"enabled""#))
        #expect(approveUpdate.body.contains(#""previousInstallId":"install-control""#))

        let approvedReport = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.install_report", "args": ["appId": "notes-lite", "installId": pendingInstallId]])
        )
        #expect(approvedReport.statusCode == 200)
        #expect(approvedReport.body.contains(#""status":"accepted""#))
        #expect(approvedReport.body.contains(#""approvalGranted":true"#))
        #expect(approvedReport.body.contains("network.request"))

        let migrationApprovalPackageURL = repoRoot
            .appendingPathComponent("native/macos/.build/data-version-update-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.copyItem(
            at: repoRoot.appendingPathComponent("webapps/examples/api-dashboard"),
            to: migrationApprovalPackageURL
        )
        defer {
            try? FileManager.default.removeItem(at: migrationApprovalPackageURL)
        }
        let migrationApprovalManifestURL = migrationApprovalPackageURL.appendingPathComponent("manifest.json")
        let migrationApprovalManifestData = try Data(contentsOf: migrationApprovalManifestURL)
        var migrationApprovalManifest = try #require(try JSONSerialization.jsonObject(with: migrationApprovalManifestData) as? [String: Any])
        migrationApprovalManifest["version"] = "0.2.0"
        migrationApprovalManifest["dataVersion"] = 2
        try jsonObjectString(migrationApprovalManifest).write(to: migrationApprovalManifestURL, atomically: true, encoding: .utf8)

        let packagedMigrationDirectory = migrationApprovalPackageURL.appendingPathComponent("migrations", isDirectory: true)
        try FileManager.default.createDirectory(at: packagedMigrationDirectory, withIntermediateDirectories: true)
        let packagedMigration: [String: Any] = [
            "appId": "api-dashboard",
            "fromDataVersion": 1,
            "toDataVersion": 2,
            "steps": [
                [
                    "op": "setDefault",
                    "key": "api-dashboard:migration-sentinel",
                    "to": "$.approved",
                    "value": true,
                ],
            ],
        ]
        try jsonObjectString(packagedMigration)
            .write(to: packagedMigrationDirectory.appendingPathComponent("1_to_2.json"), atomically: true, encoding: .utf8)

        let pendingMigrationInstall = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.install_webapp_package", "args": ["path": migrationApprovalPackageURL.path]])
        )
        #expect(pendingMigrationInstall.statusCode == 200)
        let pendingMigrationInstallResult = try jsonResult(pendingMigrationInstall)
        let pendingMigrationInstallId = try #require(pendingMigrationInstallResult["installId"] as? String)
        #expect(pendingMigrationInstallResult["status"] as? String == "requires-approval")
        let migrationApproval = try #require(pendingMigrationInstallResult["approval"] as? [String: Any])
        let migrationApprovalReasons = try #require(migrationApproval["reasons"] as? [String])
        #expect(migrationApprovalReasons.contains("dataVersion"))

        let approveMigratedUpdate = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "platform.approve_webapp_update", "args": ["appId": "api-dashboard", "installId": pendingMigrationInstallId]])
        )
        #expect(approveMigratedUpdate.statusCode == 200)
        let approveMigratedUpdateResult = try jsonResult(approveMigratedUpdate)
        #expect(approveMigratedUpdateResult["status"] as? String == "enabled")
        let packagedMigrationRuns = try #require(approveMigratedUpdateResult["migrationRuns"] as? [[String: Any]])
        #expect(packagedMigrationRuns.count == 1)
        #expect(packagedMigrationRuns.contains { run in
            (run["mode"] as? String) == "apply" && (run["status"] as? String) == "passed"
        })
        #expect(try sqliteMigrationRunCount(dbURL: dbURL, appId: "api-dashboard") >= 1)
        #expect(try sqliteAppDataVersion(dbURL: dbURL, appId: "api-dashboard") == 2)

        let migratedApprovalStorage = PlatformStorage(databaseURL: dbURL).get(BridgeRequest(
            id: "migration-approval-storage",
            method: "storage.get",
            params: ["key": "api-dashboard:migration-sentinel", "defaultValue": NSNull()],
            context: AppSandboxContext(
                appId: "api-dashboard",
                approvedPermissions: ["storage.read"],
                networkPolicy: [],
                denyPrivateNetwork: true,
                mountToken: "migration-approval-mount"
            )
        ))
        #expect(migratedApprovalStorage.ok)
        let migratedApprovalStorageResult = try #require(migratedApprovalStorage.result as? [String: Any])
        let migratedApprovalStorageValue = try #require(migratedApprovalStorageResult["value"] as? [String: Any])
        #expect(migratedApprovalStorageValue["approved"] as? Bool == true)

        let backupExport = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"db.export_backup","args":{}}"#
        )
        #expect(backupExport.statusCode == 200)
        #expect(backupExport.body.contains(#""type":"backup""#))
        #expect(backupExport.body.contains(#""contentHash":"sha256:"#))
        let backupDocument = try jsonResult(backupExport)
        let backupStorage = try #require(backupDocument["appStorage"] as? [[String: Any]])
        #expect(backupStorage.contains { row in
            (row["key"] as? String) == "core-replay-lab:state"
                && ((row["value_json"] as? String)?.contains("Delete me on uninstall") ?? false)
        })

        let quarantineCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.quarantine_webapp","args":{"appId":"core-replay-lab","reason":"control lifecycle test"}}"#
        )
        #expect(quarantineCommand.statusCode == 200)
        #expect(quarantineCommand.body.contains(#""status":"quarantined""#))
        #expect(quarantineCommand.body.contains(#""installId":"install-core-replay-control""#))

        let quarantinedOpen = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.open_webapp","args":{"appId":"core-replay-lab"}}"#
        )
        #expect(quarantinedOpen.statusCode == 400)
        #expect(quarantinedOpen.body.contains("package_quarantined"))

        let uninstallWithoutConfirm = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.uninstall_webapp","args":{"appId":"core-replay-lab"}}"#
        )
        #expect(uninstallWithoutConfirm.statusCode == 400)
        #expect(uninstallWithoutConfirm.body.contains("confirmation_required"))

        let uninstallCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.uninstall_webapp","args":{"appId":"core-replay-lab","confirm":true}}"#
        )
        #expect(uninstallCommand.statusCode == 200)
        #expect(uninstallCommand.body.contains(#""status":"uninstalled""#))
        #expect(uninstallCommand.body.contains(#""snapshotId":"snapshot_"#))
        #expect(uninstallCommand.body.contains(#""clearedStorageKeys":1"#))

        let listWithUninstalled = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.list_webapps","args":{"includeUninstalled":true}}"#
        )
        #expect(listWithUninstalled.statusCode == 200)
        #expect(listWithUninstalled.body.contains(#""appId":"core-replay-lab""#))
        #expect(listWithUninstalled.body.contains(#""status":"uninstalled""#))

        let importBackup = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString(["tool": "db.import_backup", "args": ["backup": backupDocument]])
        )
        #expect(importBackup.statusCode == 200)
        #expect(importBackup.body.contains(#""ok":true"#))
        #expect(importBackup.body.contains(#""appStorage":"#))

        let restoredCoreOpen = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.open_webapp","args":{"appId":"core-replay-lab"}}"#
        )
        #expect(restoredCoreOpen.statusCode == 200)
        #expect(restoredCoreOpen.body.contains(#""appId":"core-replay-lab""#))

        let restoredCoreStorage = PlatformStorage(databaseURL: dbURL).get(BridgeRequest(
            id: "control-import-restored-storage",
            method: "storage.get",
            params: ["key": "core-replay-lab:state", "defaultValue": NSNull()],
            context: uninstallStorageContext
        ))
        #expect(restoredCoreStorage.ok)
        let restoredCoreStorageResult = try #require(restoredCoreStorage.result as? [String: Any])
        let restoredCoreStorageValue = try #require(restoredCoreStorageResult["value"] as? [String: Any])
        #expect(restoredCoreStorageValue["title"] as? String == "Delete me on uninstall")
        #expect(try sqliteBackupExportCount(dbURL: dbURL, type: "backup") == 1)
        #expect(try sqliteBackupExportCount(dbURL: dbURL, type: "import") == 1)

        let bridgeCallsQuery = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/db/bridge-calls")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"appId":"notes-lite"}"#
        )
        #expect(bridgeCallsQuery.statusCode == 200)
        #expect(bridgeCallsQuery.body.contains(#""rows":[]"#))

        let controlStorageSet = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.storage_set","args":{"appId":"notes-lite","key":"notes-lite:control-effect","value":{"title":"Seeded by control"}}}"#
        )
        #expect(controlStorageSet.statusCode == 200)
        #expect(controlStorageSet.body.contains(#""ok":true"#))

        let controlStorageGet = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.storage_get","args":{"appId":"notes-lite","key":"notes-lite:control-effect"}}"#
        )
        #expect(controlStorageGet.statusCode == 200)
        #expect(controlStorageGet.body.contains("Seeded by control"))

        let storageAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_storage","args":{"appId":"notes-lite","key":"notes-lite:control-effect","value":{"title":"Seeded by control"}}}"#
        )
        #expect(storageAssert.statusCode == 200)
        #expect(storageAssert.body.contains(#""key":"notes-lite:control-effect""#))

        let migrationJSON = #"{"appId":"notes-lite","fromDataVersion":1,"toDataVersion":2,"steps":[{"op":"setDefault","key":"notes-lite:control-effect","to":"$.migrated","value":true}]}"#
        let migrationDryRun = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.migration_dry_run","args":{"migration":\#(migrationJSON)}}"#
        )
        #expect(migrationDryRun.statusCode == 200)
        #expect(migrationDryRun.body.contains(#""mode":"dry-run""#))
        #expect(migrationDryRun.body.contains(#""snapshotId":"snapshot_"#))
        #expect(migrationDryRun.body.contains("notes-lite:control-effect"))

        let migrationApply = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.migration_apply","args":{"migration":\#(migrationJSON)}}"#
        )
        #expect(migrationApply.statusCode == 200)
        #expect(migrationApply.body.contains(#""mode":"apply""#))
        #expect(migrationApply.body.contains(#""status":"passed""#))
        #expect(try sqliteMigrationRunCount(dbURL: dbURL, appId: "notes-lite") >= 2)
        #expect(try sqliteAppDataVersion(dbURL: dbURL, appId: "notes-lite") == 2)

        let migratedStorageGet = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.storage_get","args":{"appId":"notes-lite","key":"notes-lite:control-effect"}}"#
        )
        #expect(migratedStorageGet.statusCode == 200)
        #expect(migratedStorageGet.body.contains(#""migrated":true"#))

        let bridgeCallAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_bridge_call","args":{"appId":"notes-lite","method":"storage.set"}}"#
        )
        #expect(bridgeCallAssert.statusCode == 200)
        #expect(bridgeCallAssert.body.contains(#""method":"storage.set""#))

        let runtimeBridgeCalls = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.bridge_calls","args":{"appId":"notes-lite"}}"#
        )
        #expect(runtimeBridgeCalls.statusCode == 200)
        #expect(runtimeBridgeCalls.body.contains(#""method":"storage.get""#))
        #expect(runtimeBridgeCalls.body.contains(#""method":"storage.set""#))

        let noConsoleErrors = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_no_console_errors","args":{"appId":"notes-lite"}}"#
        )
        #expect(noConsoleErrors.statusCode == 200)
        #expect(noConsoleErrors.body.contains(#""errors":0"#))

        let clearedLogs = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.clear_logs","args":{"appId":"notes-lite"}}"#
        )
        #expect(clearedLogs.statusCode == 200)
        #expect(clearedLogs.body.contains(#""bridgeCallsCleared":3"#))

        let bridgeCallsAfterClear = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.bridge_calls","args":{"appId":"notes-lite"}}"#
        )
        #expect(bridgeCallsAfterClear.statusCode == 200)
        #expect(bridgeCallsAfterClear.body.contains(#""bridgeCalls":[]"#))

        let storageFault = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.fault_inject","args":{"appId":"notes-lite","method":"storage.get","code":"injected_storage","message":"Injected storage fault","details":{"source":"control-test"},"once":true}}"#
        )
        #expect(storageFault.statusCode == 200)
        #expect(storageFault.body.contains(#""code":"injected_storage""#))

        let faultedStorageGet = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"notes-lite","method":"storage.get","params":{"key":"notes-lite:control-effect","defaultValue":null}}}"#
        )
        #expect(faultedStorageGet.statusCode == 200)
        #expect(faultedStorageGet.body.contains(#""code":"injected_storage""#))
        #expect(faultedStorageGet.body.contains(#""faultId":"fault_"#))

        let recoveredStorageGet = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"notes-lite","method":"storage.get","params":{"key":"notes-lite:control-effect","defaultValue":null}}}"#
        )
        #expect(recoveredStorageGet.statusCode == 200)
        #expect(recoveredStorageGet.body.contains("Seeded by control"))

        let callBridgeLog = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"notes-lite","method":"app.log","params":{"level":"info","message":"Bridge harness log"}}}"#
        )
        #expect(callBridgeLog.statusCode == 200)
        #expect(callBridgeLog.body.contains(#""id":"control_call_bridge""#))

        let consoleLogs = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.console_logs","args":{"appId":"notes-lite"}}"#
        )
        #expect(consoleLogs.statusCode == 200)
        #expect(consoleLogs.body.contains("Bridge harness log"))

        let callBridgeStorageList = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"notes-lite","method":"storage.list","params":{"prefix":"notes-lite:"}}}"#
        )
        #expect(callBridgeStorageList.statusCode == 200)
        #expect(callBridgeStorageList.body.contains(#""keys":["#))

        let callBridgeToast = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"notes-lite","method":"notification.toast","params":{"message":"Saved","level":"success"}}}"#
        )
        #expect(callBridgeToast.statusCode == 200)
        #expect(callBridgeToast.body.contains(#""id":"control_call_bridge""#))

        let notificationCapture = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.notification_capture","args":{"appId":"notes-lite"}}"#
        )
        #expect(notificationCapture.statusCode == 200)
        #expect(notificationCapture.body.contains(#""message":"Saved""#))

        let networkMock = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.network_mock_set","args":{"appId":"api-dashboard","method":"GET","urlPattern":"https://api.example.com/status","response":{"status":200,"headers":{"content-type":"application/json"},"body":{"ok":true,"source":"macos-control"}}}}"#
        )
        #expect(networkMock.statusCode == 200)
        #expect(networkMock.body.contains(#""urlPattern":"https:\/\/api.example.com\/status""#))

        let networkBridge = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"api-dashboard","method":"network.request","params":{"url":"https://api.example.com/status","method":"GET","headers":{},"body":null}}}"#
        )
        #expect(networkBridge.statusCode == 200)
        #expect(networkBridge.body.contains(#""source":"macos-control""#))

        let delayedNetworkMock = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.network_mock_set","args":{"appId":"api-dashboard","method":"GET","urlPattern":"https://api.example.com/slow","response":{"status":200,"headers":{"content-type":"application/json"},"bodyText":"slow","delayMs":50}}}"#
        )
        #expect(delayedNetworkMock.statusCode == 200)

        let timedOutNetworkBridge = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"api-dashboard","method":"network.request","params":{"url":"https://api.example.com/slow","method":"GET","headers":{},"body":null,"timeoutMs":10}}}"#
        )
        #expect(timedOutNetworkBridge.statusCode == 200)
        #expect(timedOutNetworkBridge.body.contains(#""code":"timeout""#))
        #expect(timedOutNetworkBridge.body.contains(#""timeoutMs":10"#))
        #expect(timedOutNetworkBridge.body.contains(#""delayMs":50"#))

        let resetNetworkMocks = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.network_mock_reset","args":{"appId":"api-dashboard"}}"#
        )
        #expect(resetNetworkMocks.statusCode == 200)
        #expect(resetNetworkMocks.body.contains(#""cleared":2"#))

        let dialogMock = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.dialog_mock_set","args":{"appId":"file-transformer","method":"dialog.openFile","response":{"files":[{"name":"codex.txt","mimeType":"text/plain","size":5,"text":"hello"}],"cancelled":false}}}"#
        )
        #expect(dialogMock.statusCode == 200)
        #expect(dialogMock.body.contains(#""dialogType":"openFile""#))

        let dialogBridge = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.call_bridge","args":{"appId":"file-transformer","method":"dialog.openFile","params":{"accept":["text/plain"],"multiple":false}}}"#
        )
        #expect(dialogBridge.statusCode == 200)
        #expect(dialogBridge.body.contains(#""name":"codex.txt""#))

        let coreStep = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.core_step","args":{"appId":"task-workbench","event":{"type":"CreateTask","payload":{"title":"Control task"}}}}"#
        )
        #expect(coreStep.statusCode == 200)
        #expect(coreStep.body.contains(#""id":"control_core_step""#))

        let replayEvents = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.replay_events","args":{"appId":"task-workbench","events":[{"type":"CreateTask","payload":{"title":"Replay task"}}]}}"#
        )
        #expect(replayEvents.statusCode == 200)
        #expect(replayEvents.body.contains(#""appId":"task-workbench""#))
        #expect(replayEvents.body.contains(#""index":0"#))
        #expect(replayEvents.body.contains("Replay task"))

        let coreBridgeCallAssert = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.assert_bridge_call","args":{"appId":"task-workbench","method":"core.step"}}"#
        )
        #expect(coreBridgeCallAssert.statusCode == 200)
        #expect(coreBridgeCallAssert.body.contains(#""method":"core.step""#))

        let coreSnapshot = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.core_snapshot","args":{"appId":"task-workbench"}}"#
        )
        #expect(coreSnapshot.statusCode == 200)
        #expect(coreSnapshot.body.contains(#""appId":"task-workbench""#))
        #expect(coreSnapshot.body.contains(#""coreEvents":["#))
        #expect(coreSnapshot.body.contains(#""coreActions":["#))

        let coreStepResult = try jsonResult(coreStep)
        if coreStepResult["ok"] as? Bool == true {
            let coreActionAssert = try await httpRequest(
                commandURL,
                method: "POST",
                headers: ["X-Platform-Control-Token": token],
                body: #"{"tool":"runtime.assert_core_action","args":{"appId":"task-workbench","type":"Toast"}}"#
            )
            #expect(coreActionAssert.statusCode == 200)
            #expect(coreActionAssert.body.contains(#""type":"Toast""#))
        }

        let storageResetWithoutConfirm = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.storage_reset","args":{"appId":"notes-lite"}}"#
        )
        #expect(storageResetWithoutConfirm.statusCode == 400)
        #expect(storageResetWithoutConfirm.body.contains("confirmation_required"))

        let storageReset = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.storage_reset","args":{"appId":"notes-lite","confirm":true}}"#
        )
        #expect(storageReset.statusCode == 200)
        #expect(storageReset.body.contains(#""clearedStorageKeys":"#))

        let coreEventsQuery = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"db.query_core_events","args":{"appId":"notes-lite"}}"#
        )
        #expect(coreEventsQuery.statusCode == 200)
        #expect(coreEventsQuery.body.contains(#""rows":[]"#))

        let smokeRun = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"runtime.run_smoke_tests","args":{"appId":"notes-lite"}}"#
        )
        #expect(smokeRun.statusCode == 200)
        #expect(smokeRun.body.contains(#""microTestId":"smoke:notes-lite""#))
        #expect(smokeRun.body.contains(#""status":"passed""#))
        #expect(smokeRun.body.contains(#""runner":"static""#))

        let microtests = [
            ("tests/micro/api-dashboard-network.microtest.json", "api-dashboard-network"),
            ("tests/micro/calendar-planner-core-storage.microtest.json", "calendar-planner-core-storage"),
            ("tests/micro/core-replay-lab.microtest.json", "core-replay-lab-determinism"),
            ("tests/micro/file-transformer-dialog-core.microtest.json", "file-transformer-dialog-core"),
            ("tests/micro/notes-lite-create-note.microtest.json", "notes-lite-create-note"),
            ("tests/micro/task-workbench-core-storage.microtest.json", "task-workbench-core-storage"),
        ]
        for (path, id) in microtests {
            let microtestRun = try await httpRequest(
                commandURL,
                method: "POST",
                headers: ["X-Platform-Control-Token": token],
                body: #"{"tool":"runtime.run_microtest","args":{"microtestPath":"\#(path)"}}"#
            )
            #expect(microtestRun.statusCode == 200)
            #expect(microtestRun.body.contains(#""microTestId":"\#(id)""#))
            #expect(microtestRun.body.contains(#""status":"passed""#))
        }

        let platformSmokeRun = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"platform.run_platform_smoke","args":{"smokePath":"tests/platform-smoke/all-example-apps.platform-smoke.json","platform":"macos"}}"#
        )
        #expect(platformSmokeRun.statusCode == 200)
        #expect(platformSmokeRun.body.contains(#""microTestId":"platform-smoke:all-example-apps-cross-platform-smoke:macos""#))
        #expect(platformSmokeRun.body.contains(#""status":"passed""#))

        let testRunsQuery = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/db/test-runs")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"appId":"notes-lite"}"#
        )
        #expect(testRunsQuery.statusCode == 200)
        #expect(testRunsQuery.body.contains(#""micro_test_id":"smoke:notes-lite""#))
        #expect(testRunsQuery.body.contains(#""micro_test_id":"notes-lite-create-note""#))
        #expect(testRunsQuery.body.contains(#""status":"passed""#))

        let debugBundle = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/db/export-debug-bundle")!,
            method: "POST",
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(debugBundle.statusCode == 200)
        #expect(debugBundle.body.contains(#""type":"debug-bundle""#))
        #expect(debugBundle.body.contains(#""contentHash":"sha256:"#))
        #expect(debugBundle.body.contains(#""appStorage":["#))
        #expect(debugBundle.body.contains(#""runtimeCapabilities":{"#))

        let debugBundleCommand = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: #"{"tool":"db.export_debug_bundle","args":{}}"#
        )
        #expect(debugBundleCommand.statusCode == 200)
        #expect(debugBundleCommand.body.contains(#""debug":{"#))
        #expect(try sqliteBackupExportCount(dbURL: dbURL, type: "debug-bundle") == 2)

        let ended = try await httpRequest(
            URL(string: "http://127.0.0.1:\(port)/control/sessions/\(controlPlane.controlSessionId)")!,
            method: "DELETE",
            headers: ["X-Platform-Control-Token": token]
        )
        #expect(ended.statusCode == 200)
        #expect(ended.body.contains(#""status":"ended""#))

        #expect(try sqliteControlCommandCount(dbURL: dbURL, decision: "rejected") >= 1)
        #expect(try sqliteControlCommandCount(dbURL: dbURL, decision: "accepted") >= 95)
    }

    @Test("debug control mock network denials include bridge fixture detail subsets")
    func debugControlMockNetworkDenialsIncludeBridgeFixtureDetailSubsets() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-network-details-\(UUID().uuidString)", isDirectory: true)
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

        let responseLimitFixture = try bridgeFixture("invalid-network-response-too-large.json")
        let redirectFixture = try bridgeFixture("invalid-network-redirect-denied.json")
        let appId = try #require((responseLimitFixture["context"] as? [String: Any])?["appId"] as? String)
        let manifest = try bridgeFixtureManifest(responseLimitFixture)
        let registry = try PlatformAppRegistry(databaseURL: dbURL)
        try registry.installVersion(
            appId: appId,
            name: manifest["name"] as? String ?? "API Dashboard",
            version: "0.1.0-network-details",
            manifestJSON: try jsonObjectString(manifest),
            contentHash: "network-details-hash",
            installId: "install-network-details"
        )

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        let port = try #require(controlPlane.boundPort)
        let commandURL = URL(string: "http://127.0.0.1:\(port)/control/command")!

        for fixture in [responseLimitFixture, redirectFixture] {
            try await applyNetworkMocks(from: fixture, appId: appId, commandURL: commandURL, token: token)
            let bridgeCall = try await httpRequest(
                commandURL,
                method: "POST",
                headers: ["X-Platform-Control-Token": token],
                body: try jsonObjectString([
                    "tool": "runtime.call_bridge",
                    "args": [
                        "appId": appId,
                        "id": fixture["id"] as? String ?? "control_call_bridge",
                        "method": try #require(fixture["method"] as? String),
                        "params": try #require(fixture["params"] as? [String: Any]),
                    ],
                ])
            )

            #expect(bridgeCall.statusCode == 200)
            try expectBridgeDictionary(try jsonResult(bridgeCall), matches: fixture)
        }
    }

    @Test("debug control bridge validates and budgets app.log")
    func debugControlBridgeValidatesAndBudgetsAppLog() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-log-budget-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }
        let tokenURL = tempDir.appendingPathComponent("control.token")
        let dbURL = tempDir.appendingPathComponent("platform.sqlite")
        let appId = "log-budget-app"

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

        let manifest: [String: Any] = [
            "id": appId,
            "name": "Log Budget App",
            "version": "0.1.0",
            "runtimeVersion": "0.1.0",
            "entry": "index.html",
            "permissions": ["app.log"],
            "storagePrefix": "\(appId):",
            "dataVersion": 1,
            "capabilities": [
                "required": [],
                "optional": ["app.log"],
            ],
            "resourceBudget": [
                "maxLogLinesPerMinute": 1,
            ],
            "networkPolicy": [
                "allow": [],
            ],
        ]
        let registry = try PlatformAppRegistry(databaseURL: dbURL)
        try registry.installVersion(
            appId: appId,
            name: "Log Budget App",
            version: "0.1.0",
            manifestJSON: try jsonObjectString(manifest),
            contentHash: "log-budget-hash",
            installId: "install-log-budget"
        )

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        let port = try #require(controlPlane.boundPort)
        let commandURL = URL(string: "http://127.0.0.1:\(port)/control/command")!
        func callBridgeBody(method: String, params: [String: Any]) throws -> String {
            try jsonObjectString([
                "tool": "runtime.call_bridge",
                "args": [
                    "appId": appId,
                    "method": method,
                    "params": params,
                ],
            ])
        }

        let capabilities = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(method: "runtime.capabilities", params: [:])
        )
        #expect(capabilities.statusCode == 200)
        #expect(capabilities.body.contains(#""maxLogLinesPerMinute":1"#))

        let invalidLevel = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(method: "app.log", params: ["level": "verbose", "message": "Bad level"])
        )
        #expect(invalidLevel.statusCode == 200)
        #expect(invalidLevel.body.contains(#""code":"invalid_request""#))
        #expect(invalidLevel.body.contains("app.log level must be debug, info, warn, or error"))

        let firstLog = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(method: "app.log", params: ["level": "info", "message": "First log"])
        )
        #expect(firstLog.statusCode == 200)
        #expect(firstLog.body.contains(#""ok":true"#))

        let secondLog = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(method: "app.log", params: ["level": "info", "message": "Second log"])
        )
        #expect(secondLog.statusCode == 200)
        #expect(secondLog.body.contains(#""code":"resource_budget_exceeded""#))
        #expect(secondLog.body.contains(#""budget":"maxLogLinesPerMinute""#))
    }

    @Test("debug control bridge enforces bridge and network rate budgets")
    func debugControlBridgeEnforcesBridgeAndNetworkRateBudgets() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-rate-budget-\(UUID().uuidString)", isDirectory: true)
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

        let registry = try PlatformAppRegistry(databaseURL: dbURL)
        try registry.installVersion(
            appId: "bridge-budget-app",
            name: "Bridge Budget App",
            version: "0.1.0",
            manifestJSON: try jsonObjectString([
                "id": "bridge-budget-app",
                "name": "Bridge Budget App",
                "version": "0.1.0",
                "runtimeVersion": "0.1.0",
                "entry": "index.html",
                "permissions": [],
                "storagePrefix": "bridge-budget-app:",
                "dataVersion": 1,
                "capabilities": ["required": [], "optional": []],
                "resourceBudget": ["maxBridgeCallsPerMinute": 1],
                "networkPolicy": ["allow": []],
            ]),
            contentHash: "bridge-budget-hash",
            installId: "install-bridge-budget"
        )
        try registry.installVersion(
            appId: "network-budget-app",
            name: "Network Budget App",
            version: "0.1.0",
            manifestJSON: try jsonObjectString([
                "id": "network-budget-app",
                "name": "Network Budget App",
                "version": "0.1.0",
                "runtimeVersion": "0.1.0",
                "entry": "index.html",
                "permissions": ["network.request"],
                "storagePrefix": "network-budget-app:",
                "dataVersion": 1,
                "capabilities": ["required": [], "optional": ["network.request"]],
                "resourceBudget": ["maxNetworkRequestsPerMinute": 0],
                "networkPolicy": [
                    "allow": [
                        ["origin": "https://api.example.test", "methods": ["GET"], "headers": [], "maxRequestBytes": 1024]
                    ]
                ],
            ]),
            contentHash: "network-budget-hash",
            installId: "install-network-budget"
        )

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        let port = try #require(controlPlane.boundPort)
        let commandURL = URL(string: "http://127.0.0.1:\(port)/control/command")!
        func callBridgeBody(appId: String, method: String, params: [String: Any]) throws -> String {
            try jsonObjectString([
                "tool": "runtime.call_bridge",
                "args": [
                    "appId": appId,
                    "method": method,
                    "params": params,
                ],
            ])
        }

        let firstBridgeCall = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(appId: "bridge-budget-app", method: "runtime.capabilities", params: [:])
        )
        #expect(firstBridgeCall.statusCode == 200)
        #expect(firstBridgeCall.body.contains(#""ok":true"#))

        let secondBridgeCall = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(appId: "bridge-budget-app", method: "runtime.capabilities", params: [:])
        )
        #expect(secondBridgeCall.statusCode == 200)
        #expect(secondBridgeCall.body.contains(#""code":"resource_budget_exceeded""#))
        #expect(secondBridgeCall.body.contains(#""budget":"maxBridgeCallsPerMinute""#))

        let networkCall = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try callBridgeBody(
                appId: "network-budget-app",
                method: "network.request",
                params: ["url": "https://api.example.test/data", "method": "GET"]
            )
        )
        #expect(networkCall.statusCode == 200)
        #expect(networkCall.body.contains(#""code":"resource_budget_exceeded""#))
        #expect(networkCall.body.contains(#""budget":"maxNetworkRequestsPerMinute""#))
    }

    @Test("debug control bridge quarantines and restores after repeated resource budget violations")
    func debugControlBridgeQuarantinesAndRestoresAfterRepeatedResourceBudgetViolations() async throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-budget-quarantine-\(UUID().uuidString)", isDirectory: true)
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

        let appId = "budget-quarantine-app"
        let registry = try PlatformAppRegistry(databaseURL: dbURL)
        try registry.installVersion(
            appId: appId,
            name: "Budget Quarantine App",
            version: "0.1.0",
            manifestJSON: try jsonObjectString([
                "id": appId,
                "name": "Budget Quarantine App",
                "version": "0.1.0",
                "runtimeVersion": "0.1.0",
                "entry": "index.html",
                "permissions": [],
                "storagePrefix": "\(appId):",
                "dataVersion": 1,
                "capabilities": ["required": [], "optional": []],
                "resourceBudget": ["maxBridgeCallsPerMinute": 100],
                "networkPolicy": ["allow": []],
            ]),
            contentHash: "budget-quarantine-hash-v1",
            installId: "install-budget-quarantine-v1"
        )
        try registry.installVersion(
            appId: appId,
            name: "Budget Quarantine App",
            version: "0.2.0",
            manifestJSON: try jsonObjectString([
                "id": appId,
                "name": "Budget Quarantine App",
                "version": "0.2.0",
                "runtimeVersion": "0.1.0",
                "entry": "index.html",
                "permissions": [],
                "storagePrefix": "\(appId):",
                "dataVersion": 1,
                "capabilities": ["required": [], "optional": []],
                "resourceBudget": ["maxBridgeCallsPerMinute": 0],
                "networkPolicy": ["allow": []],
            ]),
            contentHash: "budget-quarantine-hash-v2",
            installId: "install-budget-quarantine-v2"
        )
        #expect(try registry.activeVersion(appId: appId)?.installId == "install-budget-quarantine-v2")

        let token = try String(contentsOf: tokenURL, encoding: .utf8)
        let port = try #require(controlPlane.boundPort)
        let commandURL = URL(string: "http://127.0.0.1:\(port)/control/command")!
        let body = try jsonObjectString([
            "tool": "runtime.call_bridge",
            "args": [
                "appId": appId,
                "method": "runtime.capabilities",
                "params": [:],
            ],
        ])

        for _ in 0..<3 {
            let response = try await httpRequest(
                commandURL,
                method: "POST",
                headers: ["X-Platform-Control-Token": token],
                body: body
            )
            #expect(response.statusCode == 200)
            #expect(response.body.contains(#""code":"resource_budget_exceeded""#))
            #expect(response.body.contains(#""budget":"maxBridgeCallsPerMinute""#))
        }

        #expect(try registry.activeVersion(appId: appId)?.installId == "install-budget-quarantine-v1")
        #expect(try sqliteAppVersionStatus(dbURL: dbURL, appId: appId, installId: "install-budget-quarantine-v2") == "quarantined")
        let events = try registry.installationEvents(appId: appId)
        #expect(events.contains { event in
            event.action == "quarantine"
                && event.installId == "install-budget-quarantine-v2"
                && event.previousInstallId == "install-budget-quarantine-v1"
                && event.actor == "macos-control-runtime"
        })
        #expect(events.contains { event in
            event.action == "rollback"
                && event.installId == "install-budget-quarantine-v1"
                && event.previousInstallId == "install-budget-quarantine-v2"
                && event.actor == "macos-control-runtime"
        })
    }

    @Test("core.step returns timeout when Forge core exceeds the host timeout")
    func coreStepReturnsTimeoutWhenForgeCoreExceedsHostTimeout() throws {
        let core = ForgeCoreBridge(stepTimeoutMilliseconds: 25) { _ in
            Thread.sleep(forTimeInterval: 0.4)
            return ["stateVersion": 1, "actions": []]
        }
        let context = AppSandboxContext(
            appId: "task-workbench",
            approvedPermissions: ["core.step"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "core-timeout-test-mount"
        )
        let startedAt = Date()
        let response = core.step(BridgeRequest(
            id: "core-timeout",
            method: "core.step",
            params: ["event": ["type": "SlowEvent"]],
            context: context
        ))
        let elapsed = Date().timeIntervalSince(startedAt)

        #expect(elapsed < 0.3)
        #expect(!response.ok)
        #expect(response.error?["code"] as? String == "timeout")
        #expect(response.error?["message"] as? String == "core.step timed out")
        let details = try #require(response.error?["details"] as? [String: Any])
        #expect(jsonInt(details["timeoutMs"]) == 25)
    }

    @Test("core.step returns real Forge output when a dylib is available")
    func coreStepReturnsRealForgeOutput() throws {
        guard let dylibPath = ProcessInfo.processInfo.environment["TERRANE_FORGE_FFI_DYLIB_FOR_TEST"],
              FileManager.default.fileExists(atPath: dylibPath)
        else {
            return
        }

        let core = ForgeCoreBridge(libraryPathOverride: dylibPath)
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

    @Test("notification.toast validates bridge contract params")
    func notificationToastValidatesBridgeContractParams() throws {
        let context = AppSandboxContext(
            appId: "notes-lite",
            approvedPermissions: ["notification.toast"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "test-mount"
        )
        let notifications = PlatformNotifications()

        let valid = notifications.toast(BridgeRequest(
            id: "toast-valid",
            method: "notification.toast",
            params: ["message": "Saved", "level": "success"],
            context: context
        ))
        #expect(valid.ok)

        let missingMessage = notifications.toast(BridgeRequest(
            id: "toast-missing-message",
            method: "notification.toast",
            params: ["level": "info"],
            context: context
        ))
        #expect(!missingMessage.ok)
        #expect(missingMessage.error?["code"] as? String == "invalid_request")

        let invalidLevel = notifications.toast(BridgeRequest(
            id: "toast-invalid-level",
            method: "notification.toast",
            params: ["message": "Saved", "level": "warn"],
            context: context
        ))
        #expect(!invalidLevel.ok)
        #expect(invalidLevel.error?["code"] as? String == "invalid_request")
        let details = try #require(invalidLevel.error?["details"] as? [String: Any])
        #expect(details["level"] as? String == "warn")
    }

    @MainActor
    @Test("file dialogs return selected files, save output, and structured cancellations")
    func fileDialogsReturnResultsAndCancellationErrors() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-dialogs-\(UUID().uuidString)", isDirectory: true)
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
    @Test("open file dialogs support multiple selection, accept filters, maxBytes, and validation errors")
    func openFileDialogsSupportDocumentedOptions() throws {
        let tempDir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("terrane-macos-open-dialog-options-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let textURL = tempDir.appendingPathComponent("notes.txt")
        try "hello".write(to: textURL, atomically: true, encoding: .utf8)
        let jsonURL = tempDir.appendingPathComponent("payload.json")
        try #"{"ok":true}"#.write(to: jsonURL, atomically: true, encoding: .utf8)
        let context = AppSandboxContext(
            appId: "file-transformer",
            approvedPermissions: ["dialog.openFile"],
            networkPolicy: [],
            denyPrivateNetwork: true,
            mountToken: "test-mount"
        )

        var observedMultiple = false
        let dialogs = PlatformDialogs(openFileURLsProvider: { multiple in
            observedMultiple = multiple
            return [textURL, jsonURL]
        })
        let opened = dialogs.openFile(BridgeRequest(
            id: "open-many",
            method: "dialog.openFile",
            params: ["accept": ["text/plain", "application/json"], "multiple": true, "maxBytes": 20],
            context: context
        ))
        #expect(opened.ok)
        #expect(observedMultiple)
        let openedResult = try #require(opened.result as? [String: Any])
        let files = try #require(openedResult["files"] as? [[String: Any]])
        #expect(files.count == 2)
        #expect(files[0]["name"] as? String == "notes.txt")
        #expect(files[0]["mime"] as? String == "text/plain")
        #expect(files[0]["size"] as? Int == 5)
        #expect(files[0]["text"] as? String == "hello")
        #expect(files[1]["name"] as? String == "payload.json")
        #expect(files[1]["mime"] as? String == "application/json")

        let oversized = dialogs.openFile(BridgeRequest(
            id: "open-over-limit",
            method: "dialog.openFile",
            params: ["multiple": true, "maxBytes": 5],
            context: context
        ))
        #expect(!oversized.ok)
        #expect(oversized.error?["code"] as? String == "quota_exceeded")

        var invalidProviderCalled = false
        let validationDialogs = PlatformDialogs(openFileURLsProvider: { _ in
            invalidProviderCalled = true
            return [textURL]
        })
        let invalidMultiple = validationDialogs.openFile(BridgeRequest(
            id: "invalid-multiple",
            method: "dialog.openFile",
            params: ["multiple": "yes"],
            context: context
        ))
        #expect(!invalidMultiple.ok)
        #expect(invalidMultiple.error?["code"] as? String == "invalid_request")
        #expect(invalidMultiple.error?["message"] as? String == "dialog.openFile multiple must be a boolean")

        let invalidAccept = validationDialogs.openFile(BridgeRequest(
            id: "invalid-accept",
            method: "dialog.openFile",
            params: ["accept": "text/plain"],
            context: context
        ))
        #expect(!invalidAccept.ok)
        #expect(invalidAccept.error?["code"] as? String == "invalid_request")
        #expect(invalidAccept.error?["message"] as? String == "dialog.openFile accept must be an array of strings")

        let invalidMaxBytes = validationDialogs.openFile(BridgeRequest(
            id: "invalid-max-bytes",
            method: "dialog.openFile",
            params: ["maxBytes": "5"],
            context: context
        ))
        #expect(!invalidMaxBytes.ok)
        #expect(invalidMaxBytes.error?["code"] as? String == "invalid_request")
        #expect(invalidMaxBytes.error?["message"] as? String == "dialog.openFile maxBytes must be a number")
        #expect(!invalidProviderCalled)
    }

    @MainActor
    @Test("WKWebView loads runtime resources and dispatches the native bridge")
    func webViewLoadsRuntimeAndDispatchesBridge() async throws {
        let bridge = WebBridge()
        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "TerranePlatformBridge")
        defer {
            contentController.removeScriptMessageHandler(forName: "TerranePlatformBridge")
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

private func sqliteBackupExportCount(dbURL: URL, type: String) throws -> Int {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return 0
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM backup_exports WHERE type = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, type, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW else {
        return 0
    }
    return Int(sqlite3_column_int(statement, 0))
}

private func sqliteMigrationRunCount(dbURL: URL, appId: String) throws -> Int {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return 0
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM migration_runs WHERE app_id = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, appId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW else {
        return 0
    }
    return Int(sqlite3_column_int(statement, 0))
}

private func sqliteAppDataVersion(dbURL: URL, appId: String) throws -> Int {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return 0
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT data_version FROM apps WHERE id = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, appId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW else {
        return 0
    }
    return Int(sqlite3_column_int(statement, 0))
}

private func sqliteAppendToAppFile(dbURL: URL, installId: String, path: String, suffix: String) throws {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        throw NSError(domain: "sqlite", code: 1)
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(
        db,
        "UPDATE app_files SET content_text = content_text || ? WHERE install_id = ? AND path = ?",
        -1,
        &statement,
        nil
    )
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, suffix, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    sqlite3_bind_text(statement, 2, installId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    sqlite3_bind_text(statement, 3, path, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_DONE, sqlite3_changes(db) == 1 else {
        throw NSError(domain: "sqlite", code: 2)
    }
}

private func sqliteAppVersionStatus(dbURL: URL, appId: String, installId: String) throws -> String {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return ""
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(db, "SELECT status FROM app_versions WHERE app_id = ? AND install_id = ?", -1, &statement, nil)
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, appId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    sqlite3_bind_text(statement, 2, installId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW,
          let pointer = sqlite3_column_text(statement, 0)
    else {
        return ""
    }
    return String(cString: pointer)
}

private struct RuntimeSessionRow {
    let status: String
    let activeAppId: String?
    let activeInstallId: String?
    let endedAt: String?
    let metadata: String
}

private func sqliteRuntimeSession(dbURL: URL, sessionId: String) throws -> RuntimeSessionRow? {
    var db: OpaquePointer?
    guard sqlite3_open(dbURL.path, &db) == SQLITE_OK else {
        return nil
    }
    defer { sqlite3_close(db) }

    var statement: OpaquePointer?
    sqlite3_prepare_v2(
        db,
        "SELECT status, active_app_id, active_install_id, ended_at, metadata_json FROM runtime_sessions WHERE session_id = ?",
        -1,
        &statement,
        nil
    )
    defer { sqlite3_finalize(statement) }
    sqlite3_bind_text(statement, 1, sessionId, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
    guard sqlite3_step(statement) == SQLITE_ROW else {
        return nil
    }
    return RuntimeSessionRow(
        status: sqliteColumnText(statement, 0) ?? "",
        activeAppId: sqliteColumnText(statement, 1),
        activeInstallId: sqliteColumnText(statement, 2),
        endedAt: sqliteColumnText(statement, 3),
        metadata: sqliteColumnText(statement, 4) ?? ""
    )
}

private func sqliteColumnText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
    guard sqlite3_column_type(statement, index) != SQLITE_NULL,
          let pointer = sqlite3_column_text(statement, index)
    else {
        return nil
    }
    return String(cString: pointer)
}

private func posixPermissions(at url: URL) throws -> Int {
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    return attributes[.posixPermissions] as? Int ?? 0
}

private struct HTTPTestResponse {
    let statusCode: Int
    let body: String
}

private func httpRequest(
    _ url: URL,
    method: String = "GET",
    headers: [String: String] = [:],
    body: String? = nil
) async throws -> HTTPTestResponse {
    var request = URLRequest(url: url)
    request.httpMethod = method
    for (name, value) in headers {
        request.setValue(value, forHTTPHeaderField: name)
    }
    if let body {
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body.data(using: .utf8)
    }
    let (data, response) = try await URLSession.shared.data(for: request)
    let httpResponse = try #require(response as? HTTPURLResponse)
    return HTTPTestResponse(
        statusCode: httpResponse.statusCode,
        body: String(data: data, encoding: .utf8) ?? ""
    )
}

private func jsonResult(_ response: HTTPTestResponse) throws -> [String: Any] {
    let data = try #require(response.body.data(using: .utf8))
    let object = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    return try #require(object["result"] as? [String: Any])
}

private func jsonInt(_ value: Any?) -> Int {
    if let value = value as? Int {
        return value
    }
    if let value = value as? NSNumber {
        return value.intValue
    }
    return 0
}

private func jsonObjectString(_ object: Any) throws -> String {
    let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    return try #require(String(data: data, encoding: .utf8))
}

private func repoRootURL() -> URL {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 {
        url.deleteLastPathComponent()
    }
    return url
}

private func bridgeFixture(_ fileName: String) throws -> [String: Any] {
    let url = repoRootURL()
        .appendingPathComponent("tests/fixtures/bridge")
        .appendingPathComponent(fileName)
    let data = try Data(contentsOf: url)
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
}

private func exampleManifest(appId: String) throws -> [String: Any] {
    let url = repoRootURL()
        .appendingPathComponent("webapps/examples")
        .appendingPathComponent(appId)
        .appendingPathComponent("manifest.json")
    let data = try Data(contentsOf: url)
    return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
}

private func bridgeFixtureManifest(_ fixture: [String: Any]) throws -> [String: Any] {
    let context = try #require(fixture["context"] as? [String: Any])
    let appId = try #require(context["appId"] as? String)
    var manifest = try exampleManifest(appId: appId)
    let preconditions = fixture["preconditions"] as? [String: Any] ?? [:]
    if let manifestPatch = preconditions["manifestPatch"] as? [String: Any] {
        for (key, value) in manifestPatch {
            manifest[key] = value
        }
    }
    if let resourceBudgetPatch = preconditions["resourceBudget"] as? [String: Any] {
        var resourceBudget = manifest["resourceBudget"] as? [String: Any] ?? [:]
        for (key, value) in resourceBudgetPatch {
            resourceBudget[key] = value
        }
        manifest["resourceBudget"] = resourceBudget
    }
    return manifest
}

private func bridgeFixtureContext(_ fixture: [String: Any]) throws -> AppSandboxContext {
    let manifest = try bridgeFixtureManifest(fixture)
    let context = try #require(fixture["context"] as? [String: Any])
    let appId = try #require(context["appId"] as? String)
    let networkPolicy = manifest["networkPolicy"] as? [String: Any] ?? [:]
    return AppSandboxContext(
        appId: appId,
        storagePrefix: manifest["storagePrefix"] as? String,
        approvedPermissions: Set(manifest["permissions"] as? [String] ?? []),
        networkPolicy: NetworkPolicyRule.fromManifest(manifest),
        denyPrivateNetwork: (networkPolicy["denyPrivateNetwork"] as? Bool) ?? true,
        resourceBudget: AppSandboxContext.resourceBudget(from: manifest),
        mountToken: "fixture-test-mount"
    )
}

private func applyNetworkMocks(
    from fixture: [String: Any],
    appId: String,
    commandURL: URL,
    token: String
) async throws {
    let preconditions = fixture["preconditions"] as? [String: Any] ?? [:]
    let networkMocks = preconditions["networkMocks"] as? [[String: Any]] ?? []
    for mock in networkMocks {
        let response = try await httpRequest(
            commandURL,
            method: "POST",
            headers: ["X-Platform-Control-Token": token],
            body: try jsonObjectString([
                "tool": "runtime.network_mock_set",
                "args": [
                    "appId": appId,
                    "method": mock["method"] as? String ?? "GET",
                    "urlPattern": try #require(mock["urlPattern"] as? String),
                    "response": mock["response"] ?? NSNull(),
                ],
            ])
        )
        #expect(response.statusCode == 200)
    }
}

private func expectBridgeDictionary(_ response: [String: Any], matches fixture: [String: Any]) throws {
    let expected = try #require(fixture["expected"] as? [String: Any])
    if let expectedOK = expected["ok"] as? Bool {
        #expect(response["ok"] as? Bool == expectedOK)
    }
    if let expectedErrorCode = expected["errorCode"] as? String {
        let error = try #require(response["error"] as? [String: Any])
        #expect(error["code"] as? String == expectedErrorCode)
        if let detailsSubset = expected["errorDetailsSubset"] as? [String: Any] {
            let details = try #require(error["details"] as? [String: Any])
            expectDictionary(details, containsSubset: detailsSubset)
        }
    }
}

private func expectDictionary(_ actual: [String: Any], containsSubset subset: [String: Any]) {
    for (key, expected) in subset {
        let actualValue = actual[key]
        if let expected = expected as? String {
            #expect(actualValue as? String == expected)
        } else if let expected = expected as? Bool {
            #expect(actualValue as? Bool == expected)
        } else if let expected = expected as? Int {
            #expect(jsonInt(actualValue) == expected)
        } else if let expected = expected as? NSNumber {
            #expect(jsonInt(actualValue) == expected.intValue)
        } else if let expected = expected as? [String: Any],
                  let actualValue = actualValue as? [String: Any] {
            expectDictionary(actualValue, containsSubset: expected)
        } else {
            #expect(Bool(false), "Unsupported bridge fixture subset value for key \(key)")
        }
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
