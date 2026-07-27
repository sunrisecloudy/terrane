import AppKit
import Foundation
import ImageIO
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

enum NativeMediaPickerError: LocalizedError, Equatable {
  case invalidOptions(String)
  case unavailable(String)
  case malformedImage
  case imageTooLarge
  case normalizationFailed
  case importFailed(String)

  var errorDescription: String? {
    switch self {
    case .invalidOptions(let message), .unavailable(let message), .importFailed(let message):
      return message
    case .malformedImage:
      return "terrane: the selected photo is malformed or could not be decoded"
    case .imageTooLarge:
      return "terrane: the selected photo exceeds the safe image size limit"
    case .normalizationFailed:
      return "terrane: the selected photo could not be normalized as JPEG"
    }
  }
}

struct NativePickOptions: Equatable {
  let source: String
  let types: [String]
  let multiple: Bool

  static func parse(_ value: Any?) throws -> NativePickOptions {
    guard let object = value as? [String: Any] else {
      throw NativeMediaPickerError.invalidOptions("terrane: pick options must be an object")
    }
    guard let source = object["source"] as? String, source == "photos" else {
      throw NativeMediaPickerError.invalidOptions(
        "terrane: pick source must be \"photos\"")
    }
    guard let types = object["types"] as? [String], types == ["image"] else {
      throw NativeMediaPickerError.invalidOptions(
        "terrane: pick types must be exactly [\"image\"]")
    }
    guard let multiple = object["multiple"] as? Bool else {
      throw NativeMediaPickerError.invalidOptions(
        "terrane: pick multiple must be a boolean")
    }
    guard !multiple else {
      throw NativeMediaPickerError.invalidOptions(
        "terrane: pick multiple=true is not supported")
    }
    return NativePickOptions(source: source, types: types, multiple: multiple)
  }
}

struct NormalizedPickerImage: Equatable {
  let data: Data
  let width: Int
  let height: Int
}

enum PickerImageNormalizer {
  static let maxSourceBytes = 40 * 1024 * 1024
  static let maxEncodedBytes = 10 * 1024 * 1024
  static let maxLongestEdge = 2_048
  static let maxSourcePixels: UInt64 = 100_000_000
  static let jpegQuality = 0.84

  static func normalize(_ sourceData: Data) throws -> NormalizedPickerImage {
    guard !sourceData.isEmpty, sourceData.count <= maxSourceBytes else {
      throw NativeMediaPickerError.imageTooLarge
    }
    guard
      let source = CGImageSourceCreateWithData(sourceData as CFData, [
        kCGImageSourceShouldCache: false
      ] as CFDictionary),
      CGImageSourceGetCount(source) == 1,
      let properties = CGImageSourceCopyPropertiesAtIndex(
        source, 0, [kCGImageSourceShouldCache: false] as CFDictionary
      ) as? [CFString: Any],
      let sourceWidth = unsignedDimension(properties[kCGImagePropertyPixelWidth]),
      let sourceHeight = unsignedDimension(properties[kCGImagePropertyPixelHeight]),
      sourceWidth > 0, sourceHeight > 0,
      sourceWidth <= UInt64(Int.max) / sourceHeight,
      sourceWidth * sourceHeight <= maxSourcePixels
    else {
      throw NativeMediaPickerError.malformedImage
    }

    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: maxLongestEdge,
      kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    else {
      throw NativeMediaPickerError.malformedImage
    }
    guard image.width > 0, image.height > 0,
      image.width <= maxLongestEdge, image.height <= maxLongestEdge
    else {
      throw NativeMediaPickerError.imageTooLarge
    }

    let output = NSMutableData()
    guard
      let destination = CGImageDestinationCreateWithData(
        output, UTType.jpeg.identifier as CFString, 1, nil)
    else {
      throw NativeMediaPickerError.normalizationFailed
    }
    // Supplying only compression and orientation properties intentionally
    // drops EXIF, GPS, camera, and Photos-library metadata.
    CGImageDestinationAddImage(
      destination,
      image,
      [
        kCGImageDestinationLossyCompressionQuality: jpegQuality,
        kCGImagePropertyOrientation: 1,
      ] as CFDictionary
    )
    guard CGImageDestinationFinalize(destination) else {
      throw NativeMediaPickerError.normalizationFailed
    }
    let data = output as Data
    guard !data.isEmpty, data.count <= maxEncodedBytes else {
      throw NativeMediaPickerError.imageTooLarge
    }
    return NormalizedPickerImage(data: data, width: image.width, height: image.height)
  }

  private static func unsignedDimension(_ value: Any?) -> UInt64? {
    if let number = value as? NSNumber {
      let signed = number.int64Value
      return signed > 0 ? UInt64(signed) : nil
    }
    return nil
  }
}

struct NativePickedBlob: Equatable {
  let name: String
  let hash: String
  let size: Int
  let mime: String
  let width: Int
  let height: Int
  let originalName: String?

  var bridgeObject: [String: Any] {
    var item: [String: Any] = [
      "kind": "blob",
      "name": name,
      "hash": hash,
      "size": size,
      "mime": mime,
      "width": width,
      "height": height,
    ]
    if let originalName, !originalName.isEmpty {
      item["originalName"] = originalName
    }
    return item
  }
}

protocol NativeBlobImporting {
  func importJPEG(_ image: NormalizedPickerImage, appId: String) throws -> NativePickedBlob
}

final class TerraneBlobImporter: NativeBlobImporting {
  private let handle: OpaquePointer

  init(handle: OpaquePointer) {
    self.handle = handle
  }

  func importJPEG(_ image: NormalizedPickerImage, appId: String) throws -> NativePickedBlob {
    guard !appId.isEmpty else {
      throw NativeMediaPickerError.importFailed("terrane: pick requires an active selected app")
    }
    let name = "imports/\(UUID().uuidString.lowercased()).jpg"
    var output: UnsafeMutablePointer<CChar>?
    var error: UnsafeMutablePointer<CChar>?
    let code = image.data.withUnsafeBytes { buffer in
      appId.withCString { app in
        name.withCString { logicalName in
          "image/jpeg".withCString { mime in
            terrane_blob_import(
              handle,
              app,
              logicalName,
              mime,
              buffer.bindMemory(to: UInt8.self).baseAddress,
              buffer.count,
              &output,
              &error
            )
          }
        }
      }
    }
    defer {
      if let output { terrane_string_free(output) }
      if let error { terrane_string_free(error) }
    }
    guard code == TERRANE_OK, let output else {
      let message = error.map { String(cString: $0) }
        ?? "terrane: selected photo could not be imported"
      throw NativeMediaPickerError.importFailed(message)
    }
    guard
      let data = String(cString: output).data(using: .utf8),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      object["kind"] as? String == "blob",
      let canonicalName = object["name"] as? String,
      let hash = object["hash"] as? String,
      let size = object["size"] as? NSNumber,
      let mime = object["mime"] as? String
    else {
      throw NativeMediaPickerError.importFailed(
        "terrane: blob importer returned a malformed descriptor")
    }
    return NativePickedBlob(
      name: canonicalName,
      hash: hash,
      size: size.intValue,
      mime: mime,
      width: image.width,
      height: image.height,
      originalName: nil
    )
  }
}

protocol NativeMediaPicking {
  func pickPhoto(
    parent: NSWindow?,
    completion: @escaping (Result<Data?, Error>) -> Void
  )
}

final class NativePickCoordinator {
  private let picker: NativeMediaPicking
  private let importer: NativeBlobImporting
  private let normalizer: (Data) throws -> NormalizedPickerImage

  init(
    picker: NativeMediaPicking,
    importer: NativeBlobImporting,
    normalizer: @escaping (Data) throws -> NormalizedPickerImage = PickerImageNormalizer.normalize
  ) {
    self.picker = picker
    self.importer = importer
    self.normalizer = normalizer
  }

  func pick(
    options: NativePickOptions,
    appId: String,
    parent: NSWindow?,
    selectedAppId: @escaping () -> String,
    completion: @escaping (Any?, String?) -> Void
  ) {
    guard options == NativePickOptions(source: "photos", types: ["image"], multiple: false) else {
      completion(nil, "terrane: unsupported pick options")
      return
    }
    picker.pickPhoto(parent: parent) { [importer, normalizer] result in
      switch result {
      case .failure(let error):
        DispatchQueue.main.async { completion(nil, error.localizedDescription) }
      case .success(nil):
        DispatchQueue.main.async {
          completion(["cancelled": true, "items": []], nil)
        }
      case .success(.some(let sourceData)):
        DispatchQueue.main.async {
          guard selectedAppId() == appId else {
            completion(nil, "terrane: selected app changed while the picker was open")
            return
          }
          DispatchQueue.global(qos: .userInitiated).async {
            do {
              let normalized = try normalizer(sourceData)
              let item = try importer.importJPEG(normalized, appId: appId)
              // Release source and normalized buffers when this bounded worker
              // scope exits; neither is retained by the picker coordinator.
              DispatchQueue.main.async {
                completion(["cancelled": false, "items": [item.bridgeObject]], nil)
              }
            } catch {
              DispatchQueue.main.async { completion(nil, error.localizedDescription) }
            }
          }
        }
      }
    }
  }
}

final class PhotosNativeMediaPicker: NativeMediaPicking {
  func pickPhoto(
    parent: NSWindow?,
    completion: @escaping (Result<Data?, Error>) -> Void
  ) {
    NativePhotosPickerController.present(parent: parent, completion: completion)
  }
}

private struct NativePhotosPickerView: View {
  @State private var selection: PhotosPickerItem?
  let onData: (Data) -> Void
  let onCancel: () -> Void
  let onError: (Error) -> Void

  var body: some View {
    VStack(spacing: 18) {
      Image(systemName: "photo.on.rectangle.angled")
        .font(.system(size: 42))
        .foregroundStyle(.secondary)
      Text("Choose one photo")
        .font(.title2)
      Text("Terrane receives only the image you select.")
        .foregroundStyle(.secondary)
      PhotosPicker(selection: $selection, matching: .images) {
        Text("Open Photos")
          .frame(minWidth: 120)
      }
      .buttonStyle(.borderedProminent)
      Button("Cancel", action: onCancel)
        .keyboardShortcut(.cancelAction)
    }
    .padding(32)
    .frame(minWidth: 420, minHeight: 280)
    .onChange(of: selection) { item in
      guard let item else { return }
      Task {
        do {
          guard let data = try await item.loadTransferable(type: Data.self) else {
            throw NativeMediaPickerError.malformedImage
          }
          onData(data)
        } catch {
          onError(error)
        }
      }
    }
  }
}

private final class NativePhotosPickerController: NSObject, NSWindowDelegate {
  private static var active: [NativePhotosPickerController] = []

  private let completion: (Result<Data?, Error>) -> Void
  private weak var parent: NSWindow?
  private var panel: NSPanel?
  private var finished = false

  static func present(
    parent: NSWindow?,
    completion: @escaping (Result<Data?, Error>) -> Void
  ) {
    let controller = NativePhotosPickerController(parent: parent, completion: completion)
    active.append(controller)
    controller.present()
  }

  private init(
    parent: NSWindow?,
    completion: @escaping (Result<Data?, Error>) -> Void
  ) {
    self.parent = parent
    self.completion = completion
  }

  private func present() {
    let view = NativePhotosPickerView(
      onData: { [weak self] data in self?.finish(.success(data)) },
      onCancel: { [weak self] in self?.finish(.success(nil)) },
      onError: { [weak self] error in self?.finish(.failure(error)) }
    )
    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 420, height: 280),
      styleMask: [.titled, .closable],
      backing: .buffered,
      defer: false
    )
    panel.title = "Photos"
    panel.contentViewController = NSHostingController(rootView: view)
    panel.delegate = self
    panel.isReleasedWhenClosed = false
    self.panel = panel
    if let parent {
      parent.beginSheet(panel)
    } else {
      panel.center()
      panel.makeKeyAndOrderFront(nil)
      NSApp.activate(ignoringOtherApps: true)
    }
  }

  func windowShouldClose(_ sender: NSWindow) -> Bool {
    finish(.success(nil))
    return false
  }

  private func finish(_ result: Result<Data?, Error>) {
    guard !finished else { return }
    finished = true
    if let panel {
      if let parent, parent.attachedSheet === panel {
        parent.endSheet(panel)
      } else {
        panel.orderOut(nil)
      }
    }
    completion(result)
    Self.active.removeAll { $0 === self }
  }
}
