import CryptoKit
import Foundation
import TerranePremiumSession
import UIKit

@MainActor
final class IOSHealthAutoSync: ObservableObject {
  @Published private(set) var status = ""

  private let client: PremiumHealthImageSyncClient

  init(client: PremiumHealthImageSyncClient) {
    self.client = client
  }

  func upload(base64: String, mime: String) async throws -> PremiumHealthImageAttachment {
    guard let data = Data(base64Encoded: base64) else {
      throw PremiumHealthImageSyncError.invalidImage
    }
    return try await upload(data: data, sourceMime: mime)
  }

  func upload(data: Data, sourceMime: String = "image/jpeg") async throws
    -> PremiumHealthImageAttachment
  {
    status = "Preparing Health photo…"
    do {
      let prepared = try HealthImagePreprocessor.prepare(data)
      status = "Uploading encrypted Health photo…"
      let attachment = try await client.upload(
        image: prepared.jpeg,
        metadata: PremiumHealthImageMetadata(
          width: prepared.width,
          height: prepared.height,
          source: sourceMime == "image/jpeg" ? "ios-health" : "ios-health-\(sourceMime)"
        )
      )
      status = "Health photo synced"
      return attachment
    } catch {
      status = "Health sync failed"
      throw error
    }
  }
}

enum HealthImagePreprocessor {
  struct Prepared {
    let jpeg: Data
    let width: Int
    let height: Int
  }

  static func prepare(_ data: Data, maximumDimension: CGFloat = 2_048) throws -> Prepared {
    guard let source = UIImage(data: data), source.size.width > 0, source.size.height > 0 else {
      throw PremiumHealthImageSyncError.invalidImage
    }
    let scale = min(1, maximumDimension / max(source.size.width, source.size.height))
    let size = CGSize(
      width: max(1, floor(source.size.width * scale)),
      height: max(1, floor(source.size.height * scale))
    )
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    format.opaque = true
    let normalized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
      UIColor.black.setFill()
      UIRectFill(CGRect(origin: .zero, size: size))
      source.draw(in: CGRect(origin: .zero, size: size))
    }
    guard let jpeg = normalized.jpegData(compressionQuality: 0.86), !jpeg.isEmpty else {
      throw PremiumHealthImageSyncError.invalidImage
    }
    return Prepared(jpeg: jpeg, width: Int(size.width), height: Int(size.height))
  }
}

enum HealthSyncKeyStoreFactory {
  static func make(environment: [String: String] = ProcessInfo.processInfo.environment)
    -> (any PremiumHealthImageKeyStore)?
  {
    #if DEBUG
      if let encoded = environment["TERRANE_E2E_HEALTH_SYNC_KEY_BASE64"],
        let key = Data(base64Encoded: encoded),
        let store = try? PremiumStaticHealthImageKeyStore(key: key)
      {
        return store
      }
    #endif
    return PremiumKeychainHealthImageKeyStore(service: "com.terrane.health.sync")
  }
}

enum HealthSyncDeviceKeyStoreFactory {
  static func make(environment: [String: String] = ProcessInfo.processInfo.environment)
    -> any PremiumHealthDeviceKeyStore
  {
    #if DEBUG
      if let encoded = environment["TERRANE_E2E_HEALTH_DEVICE_KEY_BASE64"],
        let key = Data(base64Encoded: encoded),
        let store = try? PremiumStaticHealthDeviceKeyStore(privateKey: key)
      {
        return store
      }
    #endif
    return PremiumKeychainHealthDeviceKeyStore(service: "com.terrane.health.sync")
  }
}
