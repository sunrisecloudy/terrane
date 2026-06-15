import CForgeCoreBridge
import Foundation

final class ForgeCoreBridge: @unchecked Sendable {
    private final class StepCompletion: @unchecked Sendable {
        private let lock = NSLock()
        private var completed = false

        func complete(_ response: BridgeResponse, completion: @escaping @Sendable (BridgeResponse) -> Void) {
            lock.lock()
            guard !completed else {
                lock.unlock()
                return
            }
            completed = true
            lock.unlock()
            completion(response)
        }
    }

    private final class StepResultBox: @unchecked Sendable {
        private let lock = NSLock()
        private var response: BridgeResponse?

        func store(_ response: BridgeResponse) {
            lock.lock()
            self.response = response
            lock.unlock()
        }

        func load() -> BridgeResponse? {
            lock.lock()
            defer { lock.unlock() }
            return response
        }
    }

    private final class Library {
        let path: String
        let workspaceId: String
        private let bridge: OpaquePointer

        init?(path: String, workspaceId: String) {
            guard let bridge = path.withCString({ pathPointer in
                workspaceId.withCString { workspacePointer in
                    terrane_forge_core_open_in_memory(pathPointer, workspacePointer)
                }
            }) else {
                return nil
            }
            self.path = path
            self.workspaceId = workspaceId
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
    private let stepQueue = DispatchQueue(label: "terrane.macos.forge-core-step")
    private let stepTimeoutMilliseconds: Int
    private let testStep: ((Data) throws -> Any)?
    private let workspaceId: String

    var isAvailable: Bool {
        library != nil || testStep != nil
    }

    init(
        libraryPathOverride: String? = nil,
        workspaceId: String = "macos-native",
        stepTimeoutMilliseconds: Int = 2_000,
        testStep: ((Data) throws -> Any)? = nil
    ) {
        self.workspaceId = workspaceId
        self.stepTimeoutMilliseconds = stepTimeoutMilliseconds
        self.testStep = testStep
        self.library = testStep == nil
            ? Self.loadLibrary(libraryPathOverride: libraryPathOverride, workspaceId: workspaceId)
            : nil
    }

    func step(_ request: BridgeRequest) -> BridgeResponse {
        let result = StepResultBox()
        let finished = DispatchSemaphore(value: 0)
        stepAsync(request) { response in
            result.store(response)
            finished.signal()
        }
        let deadline = DispatchTime.now() + .milliseconds(stepTimeoutMilliseconds + 500)
        guard finished.wait(timeout: deadline) == .success,
              let response = result.load()
        else {
            return timeoutResponse(id: request.id)
        }
        return response
    }

    func stepAsync(_ request: BridgeRequest, completion: @escaping @Sendable (BridgeResponse) -> Void) {
        let state = StepCompletion()
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + .milliseconds(stepTimeoutMilliseconds)) { [stepTimeoutMilliseconds] in
            state.complete(
                BridgeResponse.failure(
                    id: request.id,
                    code: "timeout",
                    message: "core.step timed out",
                    details: ["timeoutMs": stepTimeoutMilliseconds]
                ),
                completion: completion
            )
        }
        stepQueue.async { [self] in
            state.complete(stepWithoutTimeout(request), completion: completion)
        }
    }

    private func stepWithoutTimeout(_ request: BridgeRequest) -> BridgeResponse {
        guard library != nil || testStep != nil else {
            return .failure(
                id: request.id,
                code: "platform_unsupported",
                message: "core.step requires a loadable libforge_ffi.dylib"
            )
        }
        return stepWithAvailableCore(request)
    }

    private func stepWithAvailableCore(_ request: BridgeRequest) -> BridgeResponse {
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

        guard JSONSerialization.isValidJSONObject(coreInput),
              let inputData = try? JSONSerialization.data(withJSONObject: coreInput)
        else {
            return .failure(
                id: request.id,
                code: "invalid_request",
                message: "core.step params must be JSON-serializable"
            )
        }

        do {
            let result = try performStep(input: inputData, request: request, payload: coreInput)
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

    private func performStep(input: Data, request: BridgeRequest, payload: [String: Any]) throws -> Any {
        if let testStep {
            return try testStep(input)
        }
        guard let library else {
            throw ForgeCoreError.emptyOutput
        }
        return try library.handle(command: coreCommand(request: request, payload: payload))
    }

    private func coreCommand(request: BridgeRequest, payload: [String: Any]) -> [String: Any] {
        [
            "request_id": request.id ?? "macos-core-step",
            "actor": [
                "actor": "macos-host",
                "role": "owner",
            ],
            "workspace_id": workspaceId,
            "applet_id": nil as Any?,
            "name": "legacy.core_step",
            "payload": payload,
        ].compactMapValues { $0 }
    }

    private func timeoutResponse(id: String?) -> BridgeResponse {
        .failure(
            id: id,
            code: "timeout",
            message: "core.step timed out",
            details: ["timeoutMs": stepTimeoutMilliseconds]
        )
    }

    private static func loadLibrary(libraryPathOverride: String?, workspaceId: String) -> Library? {
        for path in candidateLibraryPaths(libraryPathOverride: libraryPathOverride) {
            if let library = Library(path: path, workspaceId: workspaceId) {
                return library
            }
        }
        return nil
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
        paths.append(repoRoot.appendingPathComponent("forge/target/debug/libforge_ffi.dylib").path)
        paths.append(repoRoot.appendingPathComponent("forge/target/release/libforge_ffi.dylib").path)

        if let resourceURL = Bundle.main.resourceURL {
            paths.append(resourceURL.appendingPathComponent("libforge_ffi.dylib").path)
        }

        if let frameworksURL = Bundle.main.privateFrameworksURL {
            paths.append(frameworksURL.appendingPathComponent("libforge_ffi.dylib").path)
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
