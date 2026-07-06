import AVFoundation
import AppKit

struct NativeCameraFrame {
  let data: Data
  let mime: String
  let width: Int
  let height: Int

  var dataURL: String {
    "data:\(mime);base64,\(data.base64EncodedString())"
  }
}

private struct NativeCameraStreamError: LocalizedError {
  let message: String

  var errorDescription: String? {
    message
  }
}

final class NativeCameraFrameStreamer: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
  private let session = AVCaptureSession()
  private lazy var previewLayer = AVCaptureVideoPreviewLayer(session: session)
  private let sessionQueue = DispatchQueue(label: "com.terrane.host.camera-stream.session")
  private let frameQueue = DispatchQueue(label: "com.terrane.host.camera-stream.frames")
  private let ciContext = CIContext()
  private var lastFrameAt = Date.distantPast
  private var running = false

  var onFrame: ((NativeCameraFrame) -> Void)?
  var onError: ((String) -> Void)?

  func start() {
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized:
      startAuthorized()
    case .notDetermined:
      AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
        DispatchQueue.main.async {
          granted ? self?.startAuthorized() : self?.onError?("Camera access was denied.")
        }
      }
    default:
      onError?("Camera access was denied.")
    }
  }

  func stop() {
    sessionQueue.async { [weak self] in
      guard let self else { return }
      if self.session.isRunning {
        self.session.stopRunning()
      }
      self.running = false
    }
  }

  private func startAuthorized() {
    sessionQueue.async { [weak self] in
      guard let self, !self.running else { return }
      do {
        try self.configureIfNeeded()
        self.running = true
        self.session.startRunning()
      } catch {
        DispatchQueue.main.async { self.onError?(String(describing: error)) }
      }
    }
  }

  private func configureIfNeeded() throws {
    guard session.inputs.isEmpty, session.outputs.isEmpty else { return }
    _ = previewLayer
    guard let device = Self.bestVideoDevice() else {
      throw NativeCameraStreamError(message: "No camera is available on this Mac.")
    }
    let input = try AVCaptureDeviceInput(device: device)
    guard session.canAddInput(input) else {
      throw NativeCameraStreamError(message: "Camera input is not available.")
    }
    session.addInput(input)

    let output = AVCaptureVideoDataOutput()
    output.alwaysDiscardsLateVideoFrames = true
    output.videoSettings = [
      kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA
    ]
    output.setSampleBufferDelegate(self, queue: frameQueue)
    guard session.canAddOutput(output) else {
      throw NativeCameraStreamError(message: "Camera video output is not available.")
    }
    session.addOutput(output)
  }

  func captureOutput(
    _ output: AVCaptureOutput,
    didOutput sampleBuffer: CMSampleBuffer,
    from connection: AVCaptureConnection
  ) {
    let now = Date()
    guard now.timeIntervalSince(lastFrameAt) >= 0.12 else { return }
    lastFrameAt = now
    guard let imageBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
    let ciImage = CIImage(cvPixelBuffer: imageBuffer)
    let maxDimension: CGFloat = 720
    let scale = min(1, maxDimension / max(ciImage.extent.width, ciImage.extent.height))
    let displayImage =
      scale < 1
      ? ciImage.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
      : ciImage
    guard let cgImage = ciContext.createCGImage(displayImage, from: displayImage.extent) else {
      return
    }
    let bitmap = NSBitmapImageRep(cgImage: cgImage)
    guard
      let data = bitmap.representation(
        using: .jpeg,
        properties: [.compressionFactor: 0.72]
      )
    else {
      return
    }
    let frame = NativeCameraFrame(
      data: data,
      mime: "image/jpeg",
      width: bitmap.pixelsWide,
      height: bitmap.pixelsHigh
    )
    DispatchQueue.main.async { [weak self] in
      self?.onFrame?(frame)
    }
  }

  private static func bestVideoDevice() -> AVCaptureDevice? {
    var deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera]
    if #available(macOS 14.0, *) {
      deviceTypes.append(contentsOf: [.external, .continuityCamera])
    } else {
      deviceTypes.append(.externalUnknown)
    }
    return AVCaptureDevice.DiscoverySession(
      deviceTypes: deviceTypes,
      mediaType: .video,
      position: .unspecified
    ).devices.sorted { lhs, rhs in
      if lhs.isConnected != rhs.isConnected { return lhs.isConnected && !rhs.isConnected }
      if lhs.isSuspended != rhs.isSuspended { return !lhs.isSuspended && rhs.isSuspended }
      if lhs.position != rhs.position { return lhs.position != .unspecified }
      return lhs.localizedName.localizedStandardCompare(rhs.localizedName) == .orderedAscending
    }.first
  }
}
