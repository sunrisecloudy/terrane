import CForgeCoreBridge
import Foundation

final class ForgeCoreBridge: @unchecked Sendable {
    private final class Library {
        private let bridge: OpaquePointer

        init?(linked: Bool, path: String? = nil, databaseURL: URL, workspaceId: String) {
            try? FileManager.default.createDirectory(
                at: databaseURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            if linked {
                guard let bridge = databaseURL.path.withCString({ databasePointer in
                    workspaceId.withCString { workspacePointer in
                        terrane_forge_core_open(nil, databasePointer, workspacePointer)
                    }
                }) else {
                    return nil
                }
                self.bridge = bridge
                return
            }

            guard let path,
                  let bridge = path.withCString({ pathPointer in
                      databaseURL.path.withCString { databasePointer in
                          workspaceId.withCString { workspacePointer in
                              terrane_forge_core_open(pathPointer, databasePointer, workspacePointer)
                          }
                      }
                  })
            else {
                return nil
            }
            self.bridge = bridge
        }

        deinit {
            terrane_forge_core_close(bridge)
        }

        func handle(command: [String: Any]) throws -> Any {
            guard JSONSerialization.isValidJSONObject(command),
                  let commandData = try? JSONSerialization.data(withJSONObject: command),
                  let commandJSON = String(data: commandData, encoding: .utf8)
            else {
                throw ForgeCoreError.invalidCommand
            }

            guard let outputPointer = commandJSON.withCString({ terrane_forge_core_handle_command(bridge, $0) }) else {
                throw ForgeCoreError.emptyOutput
            }
            defer {
                terrane_forge_core_free_string(bridge, outputPointer)
            }

            let output = String(cString: outputPointer)
            guard let outputData = output.data(using: .utf8),
                  let response = try JSONSerialization.jsonObject(with: outputData) as? [String: Any]
            else {
                throw ForgeCoreError.invalidResponse
            }
            guard response["ok"] as? Bool == true else {
                throw ForgeCoreError.commandFailed(response)
            }
            return response["payload"] ?? NSNull()
        }
    }

    private enum ForgeCoreError: Error, @unchecked Sendable {
        case invalidCommand
        case emptyOutput
        case invalidResponse
        case commandFailed([String: Any])
    }

    private let library: Library?
    private let workspaceId: String

    var isAvailable: Bool {
        library != nil
    }

    init(libraryPathOverride: String? = nil, workspaceId: String = "ios-native") {
        self.workspaceId = workspaceId
        self.library = Self.loadLibrary(libraryPathOverride: libraryPathOverride, workspaceId: workspaceId)
    }

    func step(_ request: BridgeRequest) -> BridgeResponse {
        guard let library else {
            return .failure(
                id: request.id,
                code: "platform_unsupported",
                message: "core.step requires a linked Forge FFI core or loadable libforge_ffi.dylib"
            )
        }

        if let requestedApp = request.params["app"] as? String,
           requestedApp != request.context.appId {
            return .failure(
                id: request.id,
                code: "permission_denied",
                message: "core.step app field does not match the channel-derived app id",
                details: ["requestedApp": requestedApp, "channelApp": request.context.appId]
            )
        }

        var coreInput = request.params
        coreInput["app"] = request.context.appId

        guard JSONSerialization.isValidJSONObject(coreInput) else {
            return .failure(
                id: request.id,
                code: "invalid_request",
                message: "core.step params must be JSON-serializable"
            )
        }

        do {
            let result = try library.handle(command: commandEnvelope(
                requestId: request.id ?? "ios-core-step",
                name: "legacy.core_step",
                payload: coreInput
            ))
            return .success(id: request.id, result: result)
        } catch ForgeCoreError.commandFailed(let response) {
            return .failure(
                id: request.id,
                code: "core_error",
                message: "legacy.core_step failed",
                details: response
            )
        } catch {
            return .failure(
                id: request.id,
                code: "core_error",
                message: "core.step returned invalid JSON"
            )
        }
    }

    private func commandEnvelope(requestId: String, name: String, payload: [String: Any]) -> [String: Any] {
        [
            "request_id": requestId,
            "actor": [
                "actor": "ios-host",
                "role": "owner",
            ],
            "workspace_id": workspaceId,
            "name": name,
            "payload": payload,
        ]
    }

    private static func loadLibrary(libraryPathOverride: String?, workspaceId: String) -> Library? {
        let databaseURL = defaultDatabaseURL()
        if let linked = Library(linked: true, databaseURL: databaseURL, workspaceId: workspaceId) {
            return linked
        }

        for path in candidateLibraryPaths(libraryPathOverride: libraryPathOverride) {
            if let library = Library(linked: false, path: path, databaseURL: databaseURL, workspaceId: workspaceId) {
                return library
            }
        }
        return nil
    }

    private static func defaultDatabaseURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return base.appendingPathComponent("Terrane/forge-workspace.sqlite")
    }

    private static func candidateLibraryPaths(libraryPathOverride: String?) -> [String] {
        var paths: [String] = []

        if let libraryPathOverride, !libraryPathOverride.isEmpty {
            paths.append(libraryPathOverride)
        }

        if let overridePath = ProcessInfo.processInfo.environment["TERRANE_FORGE_FFI_DYLIB"],
           !overridePath.isEmpty {
            paths.append(overridePath)
        }

        let repoRoot = RuntimeResourceLocator.repoRootURL()
        paths.append(repoRoot.appendingPathComponent("forge/target/aarch64-apple-ios-sim/debug/libforge_ffi.dylib").path)
        paths.append(repoRoot.appendingPathComponent("forge/target/aarch64-apple-ios-sim/release/libforge_ffi.dylib").path)

        if let resourceURL = Bundle.main.resourceURL {
            paths.append(resourceURL.appendingPathComponent("libforge_ffi.dylib").path)
        }

        var seen = Set<String>()
        return paths.filter { path in
            guard !seen.contains(path) else {
                return false
            }
            seen.insert(path)
            return true
        }
    }
}
