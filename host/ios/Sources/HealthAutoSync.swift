import CryptoKit
import Foundation
import TerranePremiumSession
import UIKit

struct IOSSubmittedHealthAnalysis: Sendable {
  let attachment: PremiumHealthImageAttachment
  let job: PremiumHealthAnalysisJob
}

struct IOSHealthAnalysisUpdate: Sendable {
  let jobID: String
  let status: String
  let failureCode: String?
  let imageBase64: String?
  let mime: String?
  let resultJSON: String?

  var bridgeValue: [String: Any] {
    var value: [String: Any] = [
      "ok": true,
      "jobId": jobID,
      "status": status,
    ]
    if let failureCode { value["failureCode"] = failureCode }
    if let imageBase64 { value["imageBase64"] = imageBase64 }
    if let mime { value["mime"] = mime }
    if let resultJSON { value["resultJson"] = resultJSON }
    return value
  }
}

@MainActor
final class IOSHealthAutoSync: ObservableObject {
  @Published private(set) var status = ""
  @Published private(set) var connections: PremiumHealthConnections?
  @Published private(set) var currentDeviceID: String?
  @Published private(set) var connectionsError = ""

  private let client: PremiumHealthImageSyncClient
  private let defaults: UserDefaults
  private let pendingDefaultsKey = "health.analysis.pending.v1"
  private var requestIDs: [String: String]

  init(client: PremiumHealthImageSyncClient, defaults: UserDefaults = .standard) {
    self.client = client
    self.defaults = defaults
    requestIDs =
      defaults.dictionary(forKey: pendingDefaultsKey) as? [String: String]
      ?? [:]
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
    try await upload(data: data, sourceMime: sourceMime, note: nil)
  }

  func submit(
    base64: String,
    mime: String,
    note: String
  ) async throws -> IOSSubmittedHealthAnalysis {
    guard let data = Data(base64Encoded: base64) else {
      throw PremiumHealthImageSyncError.invalidImage
    }
    let requestID = UUID().uuidString.lowercased()
    let attachment = try await upload(
      data: data,
      sourceMime: mime,
      note: String(note.prefix(500))
    )
    status = "Waiting for your Mac…"
    let job = try await client.createAnalysisJob(
      imageID: attachment.id,
      clientRequestID: requestID
    )
    requestIDs[job.id] = requestID
    savePendingJobs()
    status = "Sent securely to your Mac"
    return IOSSubmittedHealthAnalysis(attachment: attachment, job: job)
  }

  func analysisUpdate(jobID: String) async throws -> IOSHealthAnalysisUpdate {
    let page = try await client.analysisJobs()
    guard let job = page.jobs.first(where: { $0.id == jobID }) else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    if job.resultAvailable {
      status = "Receiving encrypted nutrition…"
      let delivery = try await client.downloadAnalysisResult(job)
      guard let resultJSON = String(data: delivery.result, encoding: .utf8) else {
        throw PremiumHealthImageSyncError.invalidResponse
      }
      status = "Nutrition ready to review"
      return IOSHealthAnalysisUpdate(
        jobID: job.id,
        status: "completed",
        failureCode: nil,
        imageBase64: delivery.image.image.base64EncodedString(),
        mime: delivery.image.metadata.mime,
        resultJSON: resultJSON
      )
    }
    let displayStatus: String
    switch job.status {
    case "leased", "processing":
      displayStatus = "Your Mac is analyzing this meal…"
    case "queued", "retry_wait":
      displayStatus = "Waiting for your connected Mac…"
    case "failed":
      displayStatus = "Nutrition analysis needs attention"
    case "cancelled":
      displayStatus = "Nutrition analysis cancelled"
    default:
      displayStatus = "Syncing nutrition status…"
    }
    status = displayStatus
    return IOSHealthAnalysisUpdate(
      jobID: job.id,
      status: job.status,
      failureCode: job.failureCode,
      imageBase64: nil,
      mime: nil,
      resultJSON: nil
    )
  }

  func acknowledge(jobID: String) async throws {
    let page = try await client.analysisJobs()
    guard let job = page.jobs.first(where: { $0.id == jobID }),
      let requestID = requestIDs[jobID]
    else {
      throw PremiumHealthImageSyncError.invalidResponse
    }
    _ = try await client.acknowledgeAnalysisJob(job, clientRequestID: requestID)
    requestIDs.removeValue(forKey: jobID)
    savePendingJobs()
    status = "Nutrition saved on this iPhone"
  }

  func pendingJobIDs() async -> [String] {
    guard let page = try? await client.analysisJobs() else {
      return Array(requestIDs.keys)
    }
    return page.jobs
      .filter { requestIDs[$0.id] != nil && $0.acknowledgedAt == nil }
      .sorted { $0.createdAt < $1.createdAt }
      .map(\.id)
  }

  func refreshConnections() async {
    do {
      async let deviceID = client.currentDeviceID()
      async let latest = client.connections()
      currentDeviceID = try await deviceID
      connections = try await latest
      connectionsError = ""
    } catch {
      connectionsError = error.localizedDescription
    }
  }

  func approveMac(deviceID: String) async {
    do {
      try await client.approveSender(deviceID: deviceID)
      await refreshConnections()
      status = "Mac and iPhone can now sync both ways"
    } catch {
      connectionsError = error.localizedDescription
    }
  }

  func upload(data: Data, sourceMime: String, note: String?) async throws
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
          source: sourceMime == "image/jpeg" ? "ios-health" : "ios-health-\(sourceMime)",
          note: note
        )
      )
      status = "Health photo synced"
      return attachment
    } catch {
      status = "Health sync failed"
      throw error
    }
  }

  private func savePendingJobs() {
    defaults.set(requestIDs, forKey: pendingDefaultsKey)
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
