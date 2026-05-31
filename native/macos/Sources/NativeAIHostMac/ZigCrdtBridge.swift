import CZigCrdtBridge
import Foundation

final class ZigCrdtBridge: @unchecked Sendable {
    private final class Library {
        let path: String
        private let bridge: OpaquePointer

        init?(path: String) {
            guard let bridge = path.withCString({ native_ai_zig_crdt_open($0) }) else {
                return nil
            }
            self.path = path
            self.bridge = bridge
        }

        deinit {
            native_ai_zig_crdt_close(bridge)
        }

        func materialize(input: Data) throws -> Any {
            try call(input: input, native_ai_zig_crdt_materialize_json)
        }

        private func call(
            input: Data,
            _ function: (
                OpaquePointer?,
                UnsafePointer<UInt8>?,
                Int,
                UnsafeMutablePointer<UnsafeMutablePointer<UInt8>?>?,
                UnsafeMutablePointer<Int>?
            ) -> Int32
        ) throws -> Any {
            var outputPointer: UnsafeMutablePointer<UInt8>?
            var outputLength = 0
            let code = input.withUnsafeBytes { rawBuffer -> Int32 in
                let inputPointer = rawBuffer.bindMemory(to: UInt8.self).baseAddress
                return function(bridge, inputPointer, input.count, &outputPointer, &outputLength)
            }

            guard code == 0 else {
                throw ZigCrdtError.callFailed(code)
            }
            guard let outputPointer else {
                throw ZigCrdtError.emptyOutput
            }
            defer {
                native_ai_zig_crdt_free_output(bridge, outputPointer, outputLength)
            }

            return try JSONSerialization.jsonObject(with: Data(bytes: outputPointer, count: outputLength))
        }
    }

    private enum ZigCrdtError: Error {
        case callFailed(Int32)
        case emptyOutput
    }

    private let library: Library?

    var isAvailable: Bool {
        library != nil
    }

    init(libraryPathOverride: String? = nil) {
        self.library = Self.loadLibrary(libraryPathOverride: libraryPathOverride)
    }

    func smokeMaterialize() -> Bool {
        guard let library else {
            return false
        }
        let input = Data("{\"frontier\":{\"version\":0}}".utf8)
        return (try? library.materialize(input: input)) != nil
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

        if let overridePath = ProcessInfo.processInfo.environment["NATIVE_AI_ZIG_CRDT_DYLIB"],
           !overridePath.isEmpty {
            paths.append(overridePath)
        }

        paths.append(
            RuntimeResourceLocator.repoRootURL()
                .appendingPathComponent("zig-crdt/zig-out/lib/libzig_crdt.dylib")
                .path
        )

        if let resourceURL = Bundle.main.resourceURL {
            paths.append(resourceURL.appendingPathComponent("libzig_crdt.dylib").path)
        }

        if let frameworksURL = Bundle.main.privateFrameworksURL {
            paths.append(frameworksURL.appendingPathComponent("libzig_crdt.dylib").path)
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
