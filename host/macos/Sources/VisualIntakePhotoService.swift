import AppKit
import Photos
import Vision

struct VisualIntakeIntent: Equatable {
  let name: String
  let confidence: Double
}

struct VisualIntakeRoutingDecision: Equatable {
  let intents: [VisualIntakeIntent]
  let evidenceCodes: [String]
  let sensitivity: String
  let recommendedAppId: String?
  let recommendedAppName: String?

  static func decide(
    classifications: [(String, Double)],
    recognizedLines: [String],
    isScreenshot: Bool,
    faceCount: Int
  ) -> VisualIntakeRoutingDecision {
    let labels = classifications.map { ($0.0.lowercased(), $0.1) }
    let lines = recognizedLines.map { $0.lowercased() }
    let joined = lines.joined(separator: " ")

    let foodTokens = [
      "food", "dish", "meal", "cuisine", "fruit", "vegetable", "bread", "dessert",
      "beverage", "drink", "restaurant", "plate", "breakfast", "lunch", "dinner",
    ]
    let foodConfidence =
      labels
      .filter { label, _ in foodTokens.contains(where: { label.contains($0) }) }
      .map(\.1)
      .max() ?? 0

    let hasInvoice = ["invoice", "amount due", "bill to", "invoice number"].contains {
      joined.contains($0)
    }
    let hasReceipt = ["receipt", "subtotal", "tax", "total", "change"].contains {
      joined.contains($0)
    }
    let currencySignals = ["$", "€", "£", "¥", "฿", "usd", "eur", "gbp", "thb"].filter {
      joined.contains($0)
    }
    let documentLike = recognizedLines.count >= 4

    var intents: [VisualIntakeIntent] = []
    var evidence: [String] = []
    var recommendedAppId: String?
    var recommendedAppName: String?

    if hasInvoice {
      intents.append(VisualIntakeIntent(name: "invoice", confidence: 0.94))
      evidence.append("ocr.invoice-cues")
      recommendedAppId = "invoice"
      recommendedAppName = "Invoice"
    } else if hasReceipt && !currencySignals.isEmpty {
      intents.append(VisualIntakeIntent(name: "receipt", confidence: 0.9))
      evidence.append("ocr.receipt-cues")
      evidence.append("ocr.currency-cue")
      recommendedAppId = "invoice"
      recommendedAppName = "Invoice"
    }

    if foodConfidence >= 0.12 {
      intents.append(
        VisualIntakeIntent(
          name: "food",
          confidence: min(0.98, max(0.65, foodConfidence))
        ))
      evidence.append("vision.food")
      if recommendedAppId == nil {
        recommendedAppId = "health"
        recommendedAppName = "Health"
      }
    }

    if isScreenshot {
      intents.append(VisualIntakeIntent(name: "screenshot", confidence: 0.99))
      evidence.append("photos.screenshot")
      if recommendedAppId == nil {
        recommendedAppId = "search-notes"
        recommendedAppName = "Search Notes"
      }
    } else if documentLike && intents.isEmpty {
      intents.append(VisualIntakeIntent(name: "document", confidence: 0.72))
      evidence.append("ocr.text-density")
      recommendedAppId = "search-notes"
      recommendedAppName = "Search Notes"
    }

    if intents.isEmpty {
      let fallback = classifications.first?.1 ?? 0.25
      intents.append(
        VisualIntakeIntent(name: "other", confidence: min(0.55, max(0.2, fallback))))
      evidence.append("vision.unhandled")
    }

    intents.sort {
      if $0.confidence == $1.confidence { return $0.name < $1.name }
      return $0.confidence > $1.confidence
    }

    let sensitivity =
      faceCount > 0 || documentLike
      ? "review-required"
      : "normal"
    if faceCount > 0 {
      evidence.append("vision.face-present")
    }

    return VisualIntakeRoutingDecision(
      intents: intents,
      evidenceCodes: Array(Set(evidence)).sorted(),
      sensitivity: sensitivity,
      recommendedAppId: recommendedAppId,
      recommendedAppName: recommendedAppName
    )
  }
}

enum VisualIntakePhotoError: LocalizedError {
  case permissionDenied
  case noImages
  case imageUnavailable
  case imageEncodingFailed
  case analysisFailed(String)

  var errorDescription: String? {
    switch self {
    case .permissionDenied:
      return "Photos access is not available. Enable it in System Settings to test Visual Intake."
    case .noImages:
      return "No image assets are available in Photos."
    case .imageUnavailable:
      return "Photos could not load this image. It may still be downloading from iCloud."
    case .imageEncodingFailed:
      return "The Photos image could not be prepared for the local preview."
    case .analysisFailed(let message):
      return "Local image analysis failed: \(message)"
    }
  }
}

final class VisualIntakePhotoService: NSObject, PHPhotoLibraryChangeObserver {
  private let queue = DispatchQueue(
    label: "com.terrane.host.visual-intake", qos: .userInitiated)
  private var fetchResult: PHFetchResult<PHAsset>?
  private var watching = false
  private var processedIdentifiers = Set<String>()
  private var recentResults: [[String: Any]] = []

  var onNewImage: (([String: Any]) -> Void)?

  deinit {
    if watching {
      PHPhotoLibrary.shared().unregisterChangeObserver(self)
    }
  }

  func requestAccessAndStart(completion: @escaping (Any?, String?) -> Void) {
    authorize { [weak self] result in
      switch result {
      case .success:
        self?.startWatching(completion: completion)
      case .failure(let error):
        DispatchQueue.main.async { completion(nil, error.localizedDescription) }
      }
    }
  }

  func analyzeLatest(completion: @escaping (Any?, String?) -> Void) {
    authorize { [weak self] result in
      guard let self else {
        DispatchQueue.main.async { completion(nil, "Visual Intake is unavailable.") }
        return
      }
      switch result {
      case .failure(let error):
        DispatchQueue.main.async { completion(nil, error.localizedDescription) }
      case .success:
        self.queue.async {
          let result = Self.fetchImages()
          guard let asset = result.lastObject else {
            DispatchQueue.main.async {
              completion(nil, VisualIntakePhotoError.noImages.localizedDescription)
            }
            return
          }
          self.analyze(asset: asset, origin: "latest", completion: completion)
        }
      }
    }
  }

  func stop(completion: @escaping (Any?, String?) -> Void) {
    queue.async {
      if self.watching {
        PHPhotoLibrary.shared().unregisterChangeObserver(self)
      }
      self.watching = false
      self.fetchResult = nil
      DispatchQueue.main.async {
        completion(["ok": true, "watching": false], nil)
      }
    }
  }

  func status(completion: @escaping (Any?, String?) -> Void) {
    queue.async {
      let authorization = Self.authorizationLabel(
        PHPhotoLibrary.authorizationStatus(for: .readWrite))
      let payload: [String: Any] = [
        "ok": true,
        "watching": self.watching,
        "authorization": authorization,
        "recent": self.recentResults,
        "localOnly": true,
      ]
      DispatchQueue.main.async { completion(payload, nil) }
    }
  }

  func photoLibraryDidChange(_ changeInstance: PHChange) {
    queue.async {
      guard self.watching, let current = self.fetchResult,
        let changes = changeInstance.changeDetails(for: current)
      else {
        return
      }
      self.fetchResult = changes.fetchResultAfterChanges
      for asset in changes.insertedObjects {
        guard self.processedIdentifiers.insert(asset.localIdentifier).inserted else {
          continue
        }
        self.analyze(asset: asset, origin: "live") { [weak self] value, error in
          guard let self else { return }
          if let result = value as? [String: Any] {
            self.queue.async {
              self.recentResults.insert(result, at: 0)
              if self.recentResults.count > 20 {
                self.recentResults.removeLast(self.recentResults.count - 20)
              }
            }
            DispatchQueue.main.async {
              self.onNewImage?(result)
            }
          } else if let error {
            DispatchQueue.main.async {
              self.onNewImage?([
                "ok": false,
                "origin": "live",
                "error": error,
                "localOnly": true,
              ])
            }
          }
        }
      }
    }
  }

  private func startWatching(completion: @escaping (Any?, String?) -> Void) {
    queue.async {
      if !self.watching {
        self.fetchResult = Self.fetchImages()
        PHPhotoLibrary.shared().register(self)
        self.watching = true
      }
      let count = self.fetchResult?.count ?? 0
      DispatchQueue.main.async {
        completion(
          [
            "ok": true,
            "watching": true,
            "baselineCount": count,
            "authorization": Self.authorizationLabel(
              PHPhotoLibrary.authorizationStatus(for: .readWrite)),
            "localOnly": true,
          ],
          nil
        )
      }
    }
  }

  private func authorize(
    completion: @escaping (Result<Void, VisualIntakePhotoError>) -> Void
  ) {
    let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
    switch status {
    case .authorized, .limited:
      completion(.success(()))
    case .notDetermined:
      PHPhotoLibrary.requestAuthorization(for: .readWrite) { next in
        if next == .authorized || next == .limited {
          completion(.success(()))
        } else {
          completion(.failure(.permissionDenied))
        }
      }
    case .denied, .restricted:
      completion(.failure(.permissionDenied))
    @unknown default:
      completion(.failure(.permissionDenied))
    }
  }

  private static func fetchImages() -> PHFetchResult<PHAsset> {
    let options = PHFetchOptions()
    options.sortDescriptors = [
      NSSortDescriptor(key: "creationDate", ascending: true)
    ]
    return PHAsset.fetchAssets(with: .image, options: options)
  }

  private func analyze(
    asset: PHAsset,
    origin: String,
    completion: @escaping (Any?, String?) -> Void
  ) {
    let options = PHImageRequestOptions()
    options.deliveryMode = .highQualityFormat
    options.resizeMode = .fast
    options.version = .current
    options.isNetworkAccessAllowed = true

    PHImageManager.default().requestImage(
      for: asset,
      targetSize: NSSize(width: 768, height: 768),
      contentMode: .aspectFit,
      options: options
    ) { image, info in
      if (info?[PHImageCancelledKey] as? Bool) == true {
        return
      }
      if (info?[PHImageResultIsDegradedKey] as? Bool) == true {
        return
      }
      guard let image else {
        DispatchQueue.main.async {
          completion(nil, VisualIntakePhotoError.imageUnavailable.localizedDescription)
        }
        return
      }
      self.queue.async {
        do {
          let result = try Self.classify(image: image, asset: asset, origin: origin)
          DispatchQueue.main.async { completion(result, nil) }
        } catch {
          DispatchQueue.main.async {
            completion(
              nil,
              VisualIntakePhotoError.analysisFailed(error.localizedDescription)
                .localizedDescription)
          }
        }
      }
    }
  }

  private static func classify(
    image: NSImage,
    asset: PHAsset,
    origin: String
  ) throws -> [String: Any] {
    var proposed = NSRect(origin: .zero, size: image.size)
    guard let cgImage = image.cgImage(forProposedRect: &proposed, context: nil, hints: nil) else {
      throw VisualIntakePhotoError.imageEncodingFailed
    }

    let classificationRequest = VNClassifyImageRequest()
    let textRequest = VNRecognizeTextRequest()
    textRequest.recognitionLevel = .fast
    textRequest.usesLanguageCorrection = false
    let faceRequest = VNDetectFaceRectanglesRequest()
    let handler = VNImageRequestHandler(cgImage: cgImage, orientation: .up)
    try handler.perform([classificationRequest, textRequest, faceRequest])

    let classifications =
      (classificationRequest.results ?? [])
      .prefix(20)
      .map { ($0.identifier, Double($0.confidence)) }
    let recognizedLines =
      Array(
        (textRequest.results ?? [])
          .compactMap { $0.topCandidates(1).first?.string }
          .prefix(100))
    let faceCount = faceRequest.results?.count ?? 0
    let isScreenshot = asset.mediaSubtypes.contains(.photoScreenshot)
    let decision = VisualIntakeRoutingDecision.decide(
      classifications: classifications,
      recognizedLines: recognizedLines,
      isScreenshot: isScreenshot,
      faceCount: faceCount
    )

    let bitmap = NSBitmapImageRep(cgImage: cgImage)
    guard let jpeg = bitmap.representation(using: .jpeg, properties: [.compressionFactor: 0.76])
    else {
      throw VisualIntakePhotoError.imageEncodingFailed
    }

    var result: [String: Any] = [
      "ok": true,
      "origin": origin,
      "dataUrl": "data:image/jpeg;base64,\(jpeg.base64EncodedString())",
      "mime": "image/jpeg",
      "width": cgImage.width,
      "height": cgImage.height,
      "capturedAtMs": Int64((asset.creationDate ?? Date()).timeIntervalSince1970 * 1000),
      "isScreenshot": isScreenshot,
      "intents": decision.intents.map {
        ["name": $0.name, "confidence": $0.confidence] as [String: Any]
      },
      "evidenceCodes": decision.evidenceCodes,
      "sensitivity": decision.sensitivity,
      "ocrLineCount": recognizedLines.count,
      "faceCount": faceCount,
      "localOnly": true,
      "externalModelInvoked": false,
      "routeExecuted": false,
    ]
    if let id = decision.recommendedAppId, let name = decision.recommendedAppName {
      result["recommendation"] = [
        "appId": id,
        "appName": name,
      ]
    }
    return result
  }

  private static func authorizationLabel(_ status: PHAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not-determined"
    case .restricted: return "restricted"
    case .denied: return "denied"
    case .authorized: return "authorized"
    case .limited: return "limited"
    @unknown default: return "unknown"
    }
  }
}
