import AppKit
import AVFoundation
import Foundation

/// Host-owned microphone capture for ambient STT. Enqueues Int16 PCM through the
/// Terrane C ABI; VAD + ASR run on the native worker thread inside `terrane-host`.
final class SttCapture {
  private let handle: OpaquePointer
  private let appId: String
  private var sessionId = ""
  private let engine = AVAudioEngine()
  private var listening = false

  var onListeningChanged: ((Bool) -> Void)?

  init(handle: OpaquePointer, appId: String) {
    self.handle = handle
    self.appId = appId
  }

  var isListening: Bool { listening }

  func start() throws {
    if listening { return }
    guard ensureMicConsent() else {
      throw SttCaptureError.consentDenied
    }
    let granted = try awaitMicAccess()
    guard granted else {
      throw SttCaptureError.micDenied
    }

    sessionId = UUID().uuidString
    let code = sessionId.withCString { sid in
      appId.withCString { app in
        terrane_stt_session_begin(handle, app, sid, 16_000)
      }
    }
    guard code == TERRANE_OK else {
      throw SttCaptureError.beginFailed(code)
    }

    let input = engine.inputNode
    let format = input.outputFormat(forBus: 0)
    input.removeTap(onBus: 0)
    input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
      guard let self, self.listening else { return }
      let pcm = Self.int16Mono(from: buffer)
      guard !pcm.isEmpty else { return }
      pcm.withUnsafeBufferPointer { ptr in
        guard let base = ptr.baseAddress else { return }
        _ = self.sessionId.withCString { sid in
          terrane_stt_push_pcm(sid, base, ptr.count)
        }
      }
    }
    try engine.start()
    listening = true
    onListeningChanged?(true)
  }

  func stop(reason: String = "stopped") {
    guard listening else { return }
    listening = false
    engine.inputNode.removeTap(onBus: 0)
    engine.stop()
    let sid = sessionId
    _ = sid.withCString { session in
      appId.withCString { app in
        reason.withCString { why in
          terrane_stt_session_end(handle, app, session, why)
        }
      }
    }
    sessionId = ""
    onListeningChanged?(false)
  }

  private func ensureMicConsent() -> Bool {
    let key = "terrane.stt.consent"
    if UserDefaults.standard.string(forKey: key) == "granted" {
      return true
    }
    let alert = NSAlert()
    alert.messageText = "Enable microphone listening?"
    alert.informativeText =
      "Terrane will capture speech for on-device transcription. Audio is processed locally; only finalized text is recorded."
    alert.addButton(withTitle: "Allow")
    alert.addButton(withTitle: "Not Now")
    let response = alert.runModal()
    if response == .alertFirstButtonReturn {
      UserDefaults.standard.set("granted", forKey: key)
      return true
    }
    return false
  }

  private func awaitMicAccess() throws -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized:
      return true
    case .notDetermined:
      var granted = false
      let sem = DispatchSemaphore(value: 0)
      AVCaptureDevice.requestAccess(for: .audio) { ok in
        granted = ok
        sem.signal()
      }
      sem.wait()
      return granted
    default:
      return false
    }
  }

  private static func int16Mono(from buffer: AVAudioPCMBuffer) -> [Int16] {
    guard let channel = buffer.floatChannelData?.pointee else { return [] }
    let count = Int(buffer.frameLength)
    var out = [Int16]()
    out.reserveCapacity(count)
    for i in 0..<count {
      var sample = channel[i]
      if sample > 1 { sample = 1 }
      if sample < -1 { sample = -1 }
      let scaled = sample < 0 ? sample * 32768 : sample * 32767
      out.append(Int16(scaled))
    }
    return out
  }
}

enum SttCaptureError: Error {
  case consentDenied
  case micDenied
  case beginFailed(Int32)
}