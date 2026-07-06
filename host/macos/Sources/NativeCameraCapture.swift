import AppKit
import AVFoundation
import CoreImage

struct NativeCameraPhoto {
  let data: Data
  let mime: String
  let width: Int
  let height: Int

  var dataURL: String {
    "data:\(mime);base64,\(data.base64EncodedString())"
  }
}

private enum NativeCameraCaptureError: LocalizedError {
  case unavailable(String)
  case denied
  case cancelled
  case conversionFailed

  var errorDescription: String? {
    switch self {
    case .unavailable(let message):
      return message
    case .denied:
      return "Camera access is disabled. Enable Terrane in System Settings > Privacy & Security > Camera."
    case .cancelled:
      return "Camera capture cancelled."
    case .conversionFailed:
      return "Captured photo could not be converted to PNG."
    }
  }
}

final class NativeCameraCaptureController: NSObject, NSWindowDelegate,
  AVCaptureVideoDataOutputSampleBufferDelegate
{
  private static var activeControllers: [NativeCameraCaptureController] = []

  private let session = AVCaptureSession()
  private let sessionQueue = DispatchQueue(label: "com.terrane.host.camera-capture")
  private let videoOutput = AVCaptureVideoDataOutput()
  private let ciContext = CIContext()
  private let completion: (Result<NativeCameraPhoto, Error>) -> Void
  private var panel: NSPanel?
  private var didFinish = false
  private var captureButton: NSButton?
  private var statusLabel: NSTextField?
  private var captureGeneration = 0
  private var isCapturing = false
  private var hasReceivedFrame = false

  static func present(
    parent: NSWindow?,
    completion: @escaping (Result<NativeCameraPhoto, Error>) -> Void
  ) {
    switch AVCaptureDevice.authorizationStatus(for: .video) {
    case .authorized:
      presentAuthorized(parent: parent, completion: completion)
    case .notDetermined:
      AVCaptureDevice.requestAccess(for: .video) { granted in
        DispatchQueue.main.async {
          if granted {
            presentAuthorized(parent: parent, completion: completion)
          } else {
            completion(.failure(NativeCameraCaptureError.denied))
          }
        }
      }
    case .denied, .restricted:
      completion(.failure(NativeCameraCaptureError.denied))
    @unknown default:
      completion(.failure(NativeCameraCaptureError.denied))
    }
  }

  private static func presentAuthorized(
    parent: NSWindow?,
    completion: @escaping (Result<NativeCameraPhoto, Error>) -> Void
  ) {
    let controller = NativeCameraCaptureController(completion: completion)
    Self.activeControllers.append(controller)
    controller.present(parent: parent)
  }

  private init(completion: @escaping (Result<NativeCameraPhoto, Error>) -> Void) {
    self.completion = completion
    super.init()
  }

  private func present(parent: NSWindow?) {
    do {
      try configureSession()
    } catch {
      finish(.failure(error))
      return
    }

    let previewView = CameraPreviewView()
    previewView.translatesAutoresizingMaskIntoConstraints = false
    previewView.previewLayer = AVCaptureVideoPreviewLayer(session: session)
    previewView.previewLayer?.videoGravity = .resizeAspectFill

    let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelClicked(_:)))
    cancel.bezelStyle = .rounded
    cancel.translatesAutoresizingMaskIntoConstraints = false

    let capture = NSButton(title: "Take Photo", target: self, action: #selector(captureClicked(_:)))
    capture.bezelStyle = .rounded
    capture.keyEquivalent = "\r"
    capture.isEnabled = false
    capture.translatesAutoresizingMaskIntoConstraints = false
    captureButton = capture

    let status = NSTextField(labelWithString: "Starting camera...")
    status.textColor = .secondaryLabelColor
    status.lineBreakMode = .byTruncatingTail
    status.translatesAutoresizingMaskIntoConstraints = false
    statusLabel = status

    let buttonRow = NSStackView(views: [status, cancel, capture])
    buttonRow.orientation = .horizontal
    buttonRow.alignment = .centerY
    buttonRow.distribution = .fill
    buttonRow.spacing = 10
    buttonRow.translatesAutoresizingMaskIntoConstraints = false
    status.setContentHuggingPriority(.defaultLow, for: .horizontal)
    cancel.setContentHuggingPriority(.required, for: .horizontal)
    capture.setContentHuggingPriority(.required, for: .horizontal)

    let content = NSView()
    content.addSubview(previewView)
    content.addSubview(buttonRow)

    NSLayoutConstraint.activate([
      previewView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
      previewView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
      previewView.topAnchor.constraint(equalTo: content.topAnchor),
      previewView.bottomAnchor.constraint(equalTo: buttonRow.topAnchor),

      buttonRow.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
      buttonRow.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -14),
      buttonRow.heightAnchor.constraint(equalToConstant: 32),
      content.widthAnchor.constraint(greaterThanOrEqualToConstant: 680),
      content.heightAnchor.constraint(greaterThanOrEqualToConstant: 500),
    ])

    let panel = NSPanel(
      contentRect: NSRect(x: 0, y: 0, width: 720, height: 540),
      styleMask: [.titled, .closable],
      backing: .buffered,
      defer: false
    )
    panel.title = "Take Photo"
    panel.contentView = content
    panel.delegate = self
    panel.isReleasedWhenClosed = false
    self.panel = panel

    sessionQueue.async { [weak self, session] in
      session.startRunning()
      DispatchQueue.main.async {
        guard let self, !self.didFinish else { return }
        if session.isRunning {
          self.statusLabel?.stringValue = "Waiting for camera frame..."
        } else {
          self.statusLabel?.stringValue = "Camera could not start"
        }
      }
    }

    if let parent {
      parent.beginSheet(panel)
    } else {
      panel.center()
      panel.makeKeyAndOrderFront(nil)
      NSApp.activate(ignoringOtherApps: true)
    }
  }

  private func configureSession() throws {
    session.beginConfiguration()
    defer { session.commitConfiguration() }

    if session.canSetSessionPreset(.photo) {
      session.sessionPreset = .photo
    }

    guard let device = Self.bestVideoDevice() else {
      throw NativeCameraCaptureError.unavailable("No camera is available on this Mac.")
    }
    DispatchQueue.main.async {
      self.statusLabel?.stringValue = "Starting \(device.localizedName)..."
    }

    let input = try AVCaptureDeviceInput(device: device)
    guard session.canAddInput(input) else {
      throw NativeCameraCaptureError.unavailable("Camera input is not available.")
    }
    session.addInput(input)

    videoOutput.alwaysDiscardsLateVideoFrames = true
    videoOutput.videoSettings = [
      kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA
    ]
    videoOutput.setSampleBufferDelegate(self, queue: sessionQueue)
    guard session.canAddOutput(videoOutput) else {
      throw NativeCameraCaptureError.unavailable("Camera video output is not available.")
    }
    session.addOutput(videoOutput)
  }

  private static func bestVideoDevice() -> AVCaptureDevice? {
    let devices: [AVCaptureDevice]
    if #available(macOS 14.0, *) {
      devices = AVCaptureDevice.DiscoverySession(
        deviceTypes: [.builtInWideAngleCamera, .external, .continuityCamera],
        mediaType: .video,
        position: .unspecified
      ).devices
    } else {
      devices = AVCaptureDevice.DiscoverySession(
        deviceTypes: [.builtInWideAngleCamera, .externalUnknown],
        mediaType: .video,
        position: .unspecified
      ).devices
    }
    return devices
      .sorted { lhs, rhs in
        if lhs.isConnected != rhs.isConnected {
          return lhs.isConnected && !rhs.isConnected
        }
        if lhs.isSuspended != rhs.isSuspended {
          return !lhs.isSuspended && rhs.isSuspended
        }
        if lhs.position != rhs.position {
          return lhs.position != .unspecified && rhs.position == .unspecified
        }
        return lhs.localizedName.localizedStandardCompare(rhs.localizedName) == .orderedAscending
      }
      .first
  }

  @objc private func cancelClicked(_ sender: NSButton) {
    finish(.failure(NativeCameraCaptureError.cancelled))
  }

  @objc private func captureClicked(_ sender: NSButton) {
    guard !isCapturing else { return }
    isCapturing = true
    captureGeneration += 1
    let generation = captureGeneration
    sender.isEnabled = false
    statusLabel?.stringValue = "Capturing..."
    sessionQueue.async { [weak self] in
      guard let self, !self.didFinish else { return }
      self.captureGeneration = generation
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
      guard let self, !self.didFinish, self.isCapturing, self.captureGeneration == generation else {
        return
      }
      self.isCapturing = false
      self.captureButton?.isEnabled = true
      self.statusLabel?.stringValue = "Camera did not return a photo. Try again."
    }
  }

  func captureOutput(
    _ output: AVCaptureOutput,
    didOutput sampleBuffer: CMSampleBuffer,
    from connection: AVCaptureConnection
  ) {
    if !hasReceivedFrame {
      hasReceivedFrame = true
      DispatchQueue.main.async {
        guard !self.didFinish else { return }
        self.statusLabel?.stringValue = "Camera ready"
        self.captureButton?.isEnabled = true
      }
    }

    guard isCapturing else { return }
    isCapturing = false
    guard let photo = pngPhoto(from: sampleBuffer) else {
      finish(.failure(NativeCameraCaptureError.conversionFailed))
      return
    }
    finish(.success(photo))
  }

  func windowWillClose(_ notification: Notification) {
    if !didFinish {
      finish(.failure(NativeCameraCaptureError.cancelled))
    }
  }

  private func finish(_ result: Result<NativeCameraPhoto, Error>) {
    guard !didFinish else { return }
    didFinish = true
    sessionQueue.async { [session] in
      if session.isRunning {
        session.stopRunning()
      }
    }

    DispatchQueue.main.async {
      if let panel = self.panel {
        if let parent = panel.sheetParent {
          parent.endSheet(panel)
        }
        panel.orderOut(nil)
      }
      Self.activeControllers.removeAll { $0 === self }
      self.completion(result)
    }
  }

  private func pngPhoto(from sampleBuffer: CMSampleBuffer) -> NativeCameraPhoto? {
    guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
      return nil
    }
    let image = CIImage(cvPixelBuffer: pixelBuffer)
    guard let cgImage = ciContext.createCGImage(image, from: image.extent) else {
      return nil
    }
    let rep = NSBitmapImageRep(cgImage: cgImage)
    guard
      let png = rep.representation(using: .png, properties: [:])
    else {
      return nil
    }
    return NativeCameraPhoto(
      data: png,
      mime: "image/png",
      width: rep.pixelsWide,
      height: rep.pixelsHigh
    )
  }
}

private final class CameraPreviewView: NSView {
  var previewLayer: AVCaptureVideoPreviewLayer? {
    didSet {
      wantsLayer = true
      layer = previewLayer
      needsLayout = true
    }
  }

  override func layout() {
    super.layout()
    previewLayer?.frame = bounds
  }
}
