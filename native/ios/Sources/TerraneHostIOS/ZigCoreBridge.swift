import CZigCoreBridge
import Foundation

final class ZigCoreBridge {
    private final class Library {
        private let bridge: OpaquePointer

        init?(linked: Bool, path: String? = nil) {
            if linked {
                guard let bridge = terrane_zig_core_open(nil) else {
                    return nil
                }
                self.bridge = bridge
                return
            }

            guard let path,
                  let bridge = path.withCString({ terrane_zig_core_open($0) })
            else {
                return nil
            }
            self.bridge = bridge
        }

        deinit {
            terrane_zig_core_close(bridge)
        }

        func step(input: Data) throws -> Any {
            var outputPointer: UnsafeMutablePointer<UInt8>?
            var outputLength = 0
            let code = input.withUnsafeBytes { rawBuffer -> Int32 in
                let inputPointer = rawBuffer.bindMemory(to: UInt8.self).baseAddress
                return terrane_zig_core_step_json(
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
                terrane_zig_core_free_output(bridge, outputPointer, outputLength)
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

    var isAvailable: Bool {
        library != nil
    }

    init() {
        self.library = Self.loadLibrary()
    }

    func step(_ request: BridgeRequest) -> BridgeResponse {
        guard let library else {
            return .failure(
                id: request.id,
                code: "platform_unsupported",
                message: "core.step requires a linked Zig core or loadable libzig_core.dylib"
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
            let result = try library.step(input: inputData)
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

    private static func loadLibrary() -> Library? {
        if let linked = Library(linked: true) {
            return linked
        }

        for path in candidateLibraryPaths() {
            if let library = Library(linked: false, path: path) {
                return library
            }
        }
        return nil
    }

    private static func candidateLibraryPaths() -> [String] {
        var paths: [String] = []

        if let overridePath = ProcessInfo.processInfo.environment["TERRANE_ZIG_CORE_DYLIB"],
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
