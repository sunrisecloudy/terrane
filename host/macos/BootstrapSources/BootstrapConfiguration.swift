import Foundation

struct BootstrapConfiguration {
  // This release key is public. The corresponding private key is intentionally
  // not part of the repository and is supplied only to release packaging.
  static let releasePublicKeyHex =
    "b8ed05d072fccf49449f2f8849445a191584aacadcdd76f41eb75ee7e3a94aa8"
  static let releaseManifestURL =
    "https://github.com/sunrisecloudy/terrane/releases/latest/download/terrane-bootstrap-manifest.json"

  let manifestURL: URL
  let publicKeyHex: String
  let storeRoot: URL
  let skipRuntimeLaunch: Bool
  let allowInsecureLocalhost: Bool
  let healthTimeout: TimeInterval
  let runtimeHome: String?
  let maximumDownloadConnections: Int
  let downloadStallTimeout: TimeInterval
  let maximumDownloadRetries: Int

  static func resolve(
    environment: [String: String] = ProcessInfo.processInfo.environment,
    applicationSupport: URL? = nil
  ) throws -> BootstrapConfiguration {
    let manifestString =
      environment["TERRANE_BOOTSTRAP_MANIFEST_URL"] ?? releaseManifestURL
    guard let manifestURL = URL(string: manifestString) else {
      throw BootstrapError.invalidConfiguration("manifest URL is invalid")
    }
    let publicKeyHex =
      environment["TERRANE_BOOTSTRAP_PUBLIC_KEY_HEX"] ?? releasePublicKeyHex
    guard publicKeyHex.count == 64, publicKeyHex.allSatisfy(\.isHexDigit) else {
      throw BootstrapError.invalidConfiguration("release public key is not configured")
    }
    let root: URL
    if let override = environment["TERRANE_BOOTSTRAP_HOME"], !override.isEmpty {
      root = URL(fileURLWithPath: override, isDirectory: true)
    } else {
      let support =
        applicationSupport
        ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
      root = support.appendingPathComponent("Terrane", isDirectory: true)
    }
    let allowLocal =
      environment["TERRANE_BOOTSTRAP_ALLOW_INSECURE_LOCALHOST"] == "1"
    let skipLaunch = environment["TERRANE_BOOTSTRAP_SKIP_LAUNCH"] == "1"
    let timeout =
      environment["TERRANE_BOOTSTRAP_HEALTH_TIMEOUT"].flatMap(TimeInterval.init) ?? 20
    let runtimeHome = environment["TERRANE_BOOTSTRAP_RUNTIME_HOME"]
      .flatMap { $0.isEmpty ? nil : $0 }
    let connections =
      environment["TERRANE_BOOTSTRAP_CONNECTIONS"].flatMap(Int.init) ?? 8
    let stallTimeout =
      environment["TERRANE_BOOTSTRAP_STALL_TIMEOUT"].flatMap(TimeInterval.init) ?? 12
    let retries =
      environment["TERRANE_BOOTSTRAP_MAX_RETRIES"].flatMap(Int.init) ?? 3
    return BootstrapConfiguration(
      manifestURL: manifestURL,
      publicKeyHex: publicKeyHex,
      storeRoot: root,
      skipRuntimeLaunch: skipLaunch,
      allowInsecureLocalhost: allowLocal,
      healthTimeout: max(1, min(timeout, 120)),
      runtimeHome: runtimeHome,
      maximumDownloadConnections: max(1, min(connections, 8)),
      downloadStallTimeout: max(2, min(stallTimeout, 120)),
      maximumDownloadRetries: max(0, min(retries, 10))
    )
  }
}
