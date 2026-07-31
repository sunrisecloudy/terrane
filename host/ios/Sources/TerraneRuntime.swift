import Foundation

protocol TerraneRuntime: AnyObject {
  var availability: RuntimeAvailability { get }
  func invoke(appID: String, verb: String, arguments: [String]) async throws -> String
  func dispatch(command: String, arguments: [String]) async throws -> String
  func readBlob(appID: String, name: String) async throws -> TerraneBlobAsset
  func crdtVersion(appID: String) async throws -> String
  func crdtExport(appID: String, sinceVersion: String) async throws -> Data
  @discardableResult func crdtMerge(appID: String, update: Data) async throws -> Bool
  func close()
}

struct TerraneBlobAsset: Equatable {
  let data: Data
  let contentType: String
}

enum RuntimeAvailability: Equatable {
  case embedded
  case unavailable(String)
}

enum TerraneRuntimeError: LocalizedError {
  case unavailable(String)
  case invocation(String)

  var errorDescription: String? {
    switch self {
    case .unavailable(let message), .invocation(let message): return message
    }
  }
}

enum TerraneRuntimeFactory {
  static func make() -> any TerraneRuntime {
    RustTerraneRuntime()
  }
}

final class RustTerraneRuntime: TerraneRuntime {
  private let queue = DispatchQueue(label: "com.terrane.ios.runtime", qos: .userInitiated)
  private var handle: OpaquePointer?
  private(set) var availability: RuntimeAvailability

  init() {
    let home = Self.homeURL()
    try? FileManager.default.createDirectory(
      at: home,
      withIntermediateDirectories: true,
      attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]
    )
    var error: UnsafeMutablePointer<CChar>?
    handle = home.path.withCString { terrane_open_with_error($0, &error) }
    if handle != nil {
      availability = .embedded
    } else {
      let message =
        error.map { String(cString: UnsafePointer($0)) }
        ?? "Terrane could not open the local workspace."
      if let error { terrane_string_free(error) }
      availability = .unavailable(message)
    }
  }

  deinit {
    close()
  }

  func invoke(appID: String, verb: String, arguments: [String]) async throws -> String {
    try await call { handle, output, error in
      let args = [verb] + arguments
      return Self.withCStringArray(args) { argc, argv in
        appID.withCString {
          terrane_host_run(handle, $0, argc, argv, output, error)
        }
      }
    }
  }

  func dispatch(command: String, arguments: [String]) async throws -> String {
    try await call { handle, output, error in
      Self.withCStringArray(arguments) { argc, argv in
        command.withCString {
          terrane_dispatch(handle, $0, argc, argv, output, error)
        }
      }
    }
  }

  func readBlob(appID: String, name: String) async throws -> TerraneBlobAsset {
    let payload = try await call { handle, output, error in
      appID.withCString { app in
        name.withCString { blobName in
          terrane_blob_read(handle, app, blobName, output, error)
        }
      }
    }
    guard
      let data = payload.data(using: .utf8),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let encoded = object["content"] as? String,
      let content = Data(base64Encoded: encoded),
      let contentType = object["contentType"] as? String,
      !contentType.isEmpty
    else {
      throw TerraneRuntimeError.invocation("Terrane returned an invalid blob response.")
    }
    return TerraneBlobAsset(data: content, contentType: contentType)
  }

  func crdtVersion(appID: String) async throws -> String {
    try await call { handle, output, error in
      appID.withCString { terrane_crdt_version(handle, $0, output, error) }
    }
  }

  func crdtExport(appID: String, sinceVersion: String) async throws -> Data {
    let encoded = try await crdtCall(
      appID: appID,
      value: sinceVersion,
      function: terrane_crdt_export
    )
    guard let update = Data(base64Encoded: encoded) else {
      throw TerraneRuntimeError.invocation("Terrane returned an invalid CRDT update.")
    }
    return update
  }

  @discardableResult
  func crdtMerge(appID: String, update: Data) async throws -> Bool {
    let result = try await crdtCall(
      appID: appID,
      value: update.base64EncodedString(),
      function: terrane_crdt_merge
    )
    return result == "changed"
  }

  func close() {
    guard let handle else { return }
    terrane_close(handle)
    self.handle = nil
  }

  private func call(
    _ body:
      @escaping (
        OpaquePointer,
        UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>,
        UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
      ) -> Int32
  ) async throws -> String {
    guard let handle else {
      if case .unavailable(let message) = availability {
        throw TerraneRuntimeError.unavailable(message)
      }
      throw TerraneRuntimeError.unavailable("Terrane runtime is closed.")
    }
    return try await withCheckedThrowingContinuation { continuation in
      queue.async {
        var output: UnsafeMutablePointer<CChar>?
        var error: UnsafeMutablePointer<CChar>?
        let status = body(handle, &output, &error)
        defer {
          if let output { terrane_string_free(output) }
          if let error { terrane_string_free(error) }
        }
        if status == TERRANE_OK {
          continuation.resume(
            returning: output.map { String(cString: UnsafePointer($0)) } ?? ""
          )
        } else {
          continuation.resume(
            throwing: TerraneRuntimeError.invocation(
              error.map { String(cString: UnsafePointer($0)) }
                ?? "Terrane runtime request failed."
            )
          )
        }
      }
    }
  }

  private func crdtCall(
    appID: String,
    value: String,
    function:
      @escaping (
        OpaquePointer?, UnsafePointer<CChar>?, UnsafePointer<CChar>?,
        UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?,
        UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
      ) -> Int32
  ) async throws -> String {
    try await call { handle, output, error in
      appID.withCString { app in
        value.withCString { function(handle, app, $0, output, error) }
      }
    }
  }

  private static func withCStringArray<T>(
    _ values: [String],
    _ body: (Int, UnsafePointer<UnsafePointer<CChar>?>?) -> T
  ) -> T {
    let storage = values.map { strdup($0) }
    defer { storage.forEach { free($0) } }
    let immutableStorage = storage.map { pointer in
      pointer.map { UnsafePointer($0) }
    }
    return immutableStorage.withUnsafeBufferPointer { buffer in
      body(buffer.count, buffer.baseAddress)
    }
  }

  private static func homeURL() -> URL {
    let root = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    return root.appendingPathComponent("Terrane", isDirectory: true)
  }
}
