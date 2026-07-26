import CryptoKit
import Foundation

struct BootstrapManifest: Codable, Equatable {
  static let formatVersion = 1
  static let signingDomain = "terrane-bootstrap-manifest-v1"
  static let maximumArtifactSize: Int64 = 2 * 1024 * 1024 * 1024

  let format: Int
  let version: String
  let architecture: String
  let artifactURL: String
  let artifactSHA256: String
  let artifactSize: Int64
  let runtimeBundleName: String
  let signature: String

  func validated(publicKeyHex: String, allowInsecureLocalhost: Bool = false) throws
    -> BootstrapManifest
  {
    guard format == Self.formatVersion else {
      throw BootstrapError.invalidManifest("unsupported format \(format)")
    }
    guard !version.isEmpty, version.count <= 128, !version.containsNewline else {
      throw BootstrapError.invalidManifest("invalid runtime version")
    }
    guard architecture == "arm64" else {
      throw BootstrapError.invalidManifest("runtime architecture must be arm64")
    }
    guard artifactSHA256.count == 64, artifactSHA256.allSatisfy(\.isHexDigit) else {
      throw BootstrapError.invalidManifest("artifact SHA-256 must be 64 hexadecimal characters")
    }
    guard artifactSize > 0, artifactSize <= Self.maximumArtifactSize else {
      throw BootstrapError.invalidManifest("artifact size is outside the supported range")
    }
    guard runtimeBundleName.hasSuffix(".app"),
      runtimeBundleName.count <= 128,
      runtimeBundleName == URL(fileURLWithPath: runtimeBundleName).lastPathComponent,
      !runtimeBundleName.containsNewline
    else {
      throw BootstrapError.invalidManifest("runtime bundle name is unsafe")
    }
    guard !artifactURL.containsNewline,
      let url = URL(string: artifactURL), url.fragment == nil, url.user == nil,
      url.password == nil
    else {
      throw BootstrapError.invalidManifest("artifact URL is invalid")
    }
    let localHTTP =
      allowInsecureLocalhost && url.scheme == "http"
      && (url.host == "127.0.0.1" || url.host == "localhost" || url.host == "::1")
    guard url.scheme == "https" || localHTTP else {
      throw BootstrapError.invalidManifest("artifact URL must use HTTPS")
    }
    guard
      let keyData = Data(hex: publicKeyHex),
      keyData.count == 32,
      let signatureData = Data(base64Encoded: signature),
      signatureData.count == 64
    else {
      throw BootstrapError.invalidManifest("release signature or public key is malformed")
    }
    let publicKey: Curve25519.Signing.PublicKey
    do {
      publicKey = try Curve25519.Signing.PublicKey(rawRepresentation: keyData)
    } catch {
      throw BootstrapError.invalidManifest("release public key is invalid")
    }
    guard publicKey.isValidSignature(signatureData, for: signingPayload) else {
      throw BootstrapError.invalidSignature
    }
    return self
  }

  var signingPayload: Data {
    [
      Self.signingDomain,
      String(format),
      version,
      architecture,
      artifactURL,
      artifactSHA256.lowercased(),
      String(artifactSize),
      runtimeBundleName,
      "",
    ].joined(separator: "\n").data(using: .utf8)!
  }
}

enum BootstrapError: LocalizedError {
  case invalidConfiguration(String)
  case invalidManifest(String)
  case invalidSignature
  case download(String)
  case artifactSize(expected: Int64, actual: Int64)
  case artifactHash(expected: String, actual: String)
  case installation(String)
  case launch(String)

  var errorDescription: String? {
    switch self {
    case .invalidConfiguration(let message):
      return "Bootstrap configuration is invalid: \(message)"
    case .invalidManifest(let message):
      return "The Terrane release manifest is invalid: \(message)"
    case .invalidSignature:
      return "The Terrane release signature could not be verified."
    case .download(let message):
      return "Terrane could not be downloaded: \(message)"
    case .artifactSize(let expected, let actual):
      return
        "The download size did not match the release manifest (\(actual) of \(expected) bytes)."
    case .artifactHash:
      return "The downloaded Terrane runtime failed its integrity check."
    case .installation(let message):
      return "Terrane could not be installed: \(message)"
    case .launch(let message):
      return "Terrane could not be opened: \(message)"
    }
  }
}

extension String {
  fileprivate var containsNewline: Bool {
    contains("\n") || contains("\r")
  }
}

extension Data {
  init?(hex: String) {
    guard hex.count.isMultiple(of: 2) else { return nil }
    var data = Data(capacity: hex.count / 2)
    var index = hex.startIndex
    while index < hex.endIndex {
      let next = hex.index(index, offsetBy: 2)
      guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
      data.append(byte)
      index = next
    }
    self = data
  }
}
