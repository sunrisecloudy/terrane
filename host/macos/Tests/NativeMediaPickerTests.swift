import AppKit
import ImageIO
import UniformTypeIdentifiers
import XCTest

final class NativeMediaPickerTests: XCTestCase {
  func testSupportedOptionsParseAndUnsupportedVariantsReject() throws {
    XCTAssertEqual(
      try NativePickOptions.parse([
        "source": "photos",
        "types": ["image"],
        "multiple": false,
      ]),
      NativePickOptions(source: "photos", types: ["image"], multiple: false)
    )

    let invalidOptions: [Any] = [
      NSNull() as Any,
      ["source": "files", "types": ["image"], "multiple": false] as [String: Any],
      ["source": "photos", "types": ["video"], "multiple": false] as [String: Any],
      ["source": "photos", "types": ["image"], "multiple": true] as [String: Any],
      ["source": "photos", "types": ["image"]] as [String: Any],
    ]
    for options in invalidOptions {
      XCTAssertThrowsError(try NativePickOptions.parse(options))
    }
  }

  func testNormalizationProducesBoundedMetadataFreeJpeg() throws {
    let source = try sourcePNG(width: 3_000, height: 1_500)
    let normalized = try PickerImageNormalizer.normalize(source)

    XCTAssertEqual(normalized.width, 2_048)
    XCTAssertEqual(normalized.height, 1_024)
    XCTAssertLessThanOrEqual(normalized.data.count, PickerImageNormalizer.maxEncodedBytes)

    let imageSource = try XCTUnwrap(
      CGImageSourceCreateWithData(normalized.data as CFData, nil))
    XCTAssertEqual(
      CGImageSourceGetType(imageSource) as String?,
      UTType.jpeg.identifier
    )
    let properties = try XCTUnwrap(
      CGImageSourceCopyPropertiesAtIndex(imageSource, 0, nil) as? [CFString: Any])
    XCTAssertNil(properties[kCGImagePropertyGPSDictionary])
    XCTAssertEqual((properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue, 2_048)
    XCTAssertEqual((properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue, 1_024)
  }

  func testNormalizerRejectsEmptyAndOversizedEncodedInput() {
    XCTAssertThrowsError(try PickerImageNormalizer.normalize(Data()))
    XCTAssertThrowsError(
      try PickerImageNormalizer.normalize(
        Data(count: PickerImageNormalizer.maxSourceBytes + 1)))
  }

  func testCoordinatorMapsCancellationWithoutImporting() {
    let picker = FakePicker(result: .success(nil))
    let importer = FakeImporter()
    let coordinator = NativePickCoordinator(
      picker: picker,
      importer: importer,
      normalizer: { _ in XCTFail("normalizer should not run"); throw TestError.unexpected }
    )
    let done = expectation(description: "cancelled")
    coordinator.pick(
      options: NativePickOptions(source: "photos", types: ["image"], multiple: false),
      appId: "health",
      parent: nil,
      selectedAppId: { "health" }
    ) { object, error in
      XCTAssertNil(error)
      let response = object as? [String: Any]
      XCTAssertEqual(response?["cancelled"] as? Bool, true)
      XCTAssertEqual((response?["items"] as? [Any])?.count, 0)
      XCTAssertEqual(importer.callCount, 0)
      done.fulfill()
    }
    wait(for: [done], timeout: 1)
  }

  func testCoordinatorReturnsStableBlobDescriptor() {
    let picker = FakePicker(result: .success(Data([1, 2, 3])))
    let importer = FakeImporter()
    let coordinator = NativePickCoordinator(
      picker: picker,
      importer: importer,
      normalizer: { _ in
        NormalizedPickerImage(data: Data([0xff, 0xd8, 0xff, 0xd9]), width: 640, height: 480)
      }
    )
    let done = expectation(description: "picked")
    coordinator.pick(
      options: NativePickOptions(source: "photos", types: ["image"], multiple: false),
      appId: "health",
      parent: nil,
      selectedAppId: { "health" }
    ) { object, error in
      XCTAssertNil(error)
      let response = object as? [String: Any]
      XCTAssertEqual(response?["cancelled"] as? Bool, false)
      let item = (response?["items"] as? [[String: Any]])?.first
      XCTAssertEqual(item?["kind"] as? String, "blob")
      XCTAssertEqual(item?["name"] as? String, FakeImporter.item.name)
      XCTAssertEqual(item?["mime"] as? String, "image/jpeg")
      XCTAssertEqual(item?["width"] as? Int, 640)
      XCTAssertEqual(item?["height"] as? Int, 480)
      XCTAssertNil(item?["path"])
      XCTAssertNil(item?["assetIdentifier"])
      done.fulfill()
    }
    wait(for: [done], timeout: 2)
  }

  private func sourcePNG(width: Int, height: Int) throws -> Data {
    let colorSpace = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
    let context = try XCTUnwrap(
      CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
      ))
    context.setFillColor(NSColor.systemOrange.cgColor)
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    let image = try XCTUnwrap(context.makeImage())
    let data = NSMutableData()
    let destination = try XCTUnwrap(
      CGImageDestinationCreateWithData(
        data, UTType.png.identifier as CFString, 1, nil))
    CGImageDestinationAddImage(
      destination,
      image,
      [
        kCGImagePropertyGPSDictionary: [
          kCGImagePropertyGPSLatitude: 13.7563,
          kCGImagePropertyGPSLongitude: 100.5018,
        ]
      ] as CFDictionary
    )
    XCTAssertTrue(CGImageDestinationFinalize(destination))
    return data as Data
  }
}

private enum TestError: Error {
  case unexpected
}

private final class FakePicker: NativeMediaPicking {
  let result: Result<Data?, Error>

  init(result: Result<Data?, Error>) {
    self.result = result
  }

  func pickPhoto(
    parent: NSWindow?,
    completion: @escaping (Result<Data?, Error>) -> Void
  ) {
    completion(result)
  }
}

private final class FakeImporter: NativeBlobImporting {
  static let item = NativePickedBlob(
    name: "imports/123e4567-e89b-12d3-a456-426614174000.jpg",
    hash: String(repeating: "a", count: 64),
    size: 4,
    mime: "image/jpeg",
    width: 640,
    height: 480,
    originalName: nil
  )
  private(set) var callCount = 0

  func importJPEG(_ image: NormalizedPickerImage, appId: String) throws -> NativePickedBlob {
    callCount += 1
    return Self.item
  }
}
