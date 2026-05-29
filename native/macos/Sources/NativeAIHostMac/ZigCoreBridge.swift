import CZigCoreBridge
import Foundation

final class ZigCoreBridge: @unchecked Sendable {
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
        private let bridge: OpaquePointer

        init?(path: String) {
            guard let bridge = path.withCString({ native_ai_zig_core_open($0) }) else {
                return nil
            }
            self.path = path
            self.bridge = bridge
        }

        deinit {
            native_ai_zig_core_close(bridge)
        }

        func step(input: Data) throws -> Any {
            var outputPointer: UnsafeMutablePointer<UInt8>?
            var outputLength = 0
            let code = input.withUnsafeBytes { rawBuffer -> Int32 in
                let inputPointer = rawBuffer.bindMemory(to: UInt8.self).baseAddress
                return native_ai_zig_core_step_json(
                    bridge,
                    inputPointer,
                    input.count,
                    &outputPointer,
                    &outputLength
                )
            }

            guard code == 0 else {
                throw ZigCoreError.stepFailed(code)
            }
            guard let outputPointer else {
                throw ZigCoreError.emptyOutput
            }
            defer {
                native_ai_zig_core_free_output(bridge, outputPointer, outputLength)
            }

            let outputData = Data(bytes: outputPointer, count: outputLength)
            return try JSONSerialization.jsonObject(with: outputData)
        }
    }

    private enum ZigCoreError: Error {
        case stepFailed(Int32)
        case emptyOutput
    }

    private let library: Library?
    private let stepQueue = DispatchQueue(label: "native-ai.macos.zig-core-step")
    private let stepTimeoutMilliseconds: Int
    private let testStep: ((Data) throws -> Any)?

    var isAvailable: Bool {
        library != nil || testStep != nil
    }

    init(
        libraryPathOverride: String? = nil,
        stepTimeoutMilliseconds: Int = 2_000,
        testStep: ((Data) throws -> Any)? = nil
    ) {
        self.stepTimeoutMilliseconds = stepTimeoutMilliseconds
        self.testStep = testStep
        self.library = testStep == nil ? Self.loadLibrary(libraryPathOverride: libraryPathOverride) : nil
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
        guard let library else {
            guard testStep != nil else {
                return .failure(
                    id: request.id,
                    code: "platform_unsupported",
                    message: "core.step requires a loadable libzig_core.dylib"
                )
            }
            return stepWithAvailableCore(request)
        }
        return stepWithAvailableCore(request, library: library)
    }

    private func stepWithAvailableCore(_ request: BridgeRequest, library: Library? = nil) -> BridgeResponse {
        guard library != nil || testStep != nil else {
            return .failure(
                id: request.id,
                code: "platform_unsupported",
                message: "core.step requires a loadable libzig_core.dylib"
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
            let result = try performStep(input: inputData, library: library)
            return .success(id: request.id, result: result)
        } catch ZigCoreError.stepFailed(let code) {
            return .failure(
                id: request.id,
                code: "core_error",
                message: "core_step_json failed",
                details: ["status": Int(code)]
            )
        } catch {
            return .failure(
                id: request.id,
                code: "core_error",
                message: "core.step returned invalid JSON"
            )
        }
    }

    private func performStep(input: Data, library: Library?) throws -> Any {
        if let testStep {
            return try testStep(input)
        }
        guard let library else {
            throw ZigCoreError.emptyOutput
        }
        return try library.step(input: input)
    }

    private func timeoutResponse(id: String?) -> BridgeResponse {
        .failure(
            id: id,
            code: "timeout",
            message: "core.step timed out",
            details: ["timeoutMs": stepTimeoutMilliseconds]
        )
    }

    private static func loadLibrary(libraryPathOverride: String?) -> Library? {
        for path in candidateLibraryPaths(libraryPathOverride: libraryPathOverride) {
            if let library = Library(path: path) {
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

        if let overridePath = ProcessInfo.processInfo.environment["NATIVE_AI_ZIG_CORE_DYLIB"],
           !overridePath.isEmpty {
            paths.append(overridePath)
        }

        paths.append(
            RuntimeResourceLocator.repoRootURL()
                .appendingPathComponent("zig-core/zig-out/lib/libzig_core.dylib")
                .path
        )

        if let resourceURL = Bundle.main.resourceURL {
            paths.append(resourceURL.appendingPathComponent("libzig_core.dylib").path)
        }

        if let frameworksURL = Bundle.main.privateFrameworksURL {
            paths.append(frameworksURL.appendingPathComponent("libzig_core.dylib").path)
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
