import Foundation

struct AppConfiguration {
  let premiumBaseURL: URL?
  let googleClientID: String?
  let googleServerClientID: String?

  static let current = AppConfiguration(bundle: .main)

  init(bundle: Bundle) {
    #if DEBUG
      let premiumURLValue =
        ProcessInfo.processInfo.environment["TERRANE_E2E_PREMIUM_URL"]
        ?? bundle.object(forInfoDictionaryKey: "TerranePremiumBaseURL")
    #else
      let premiumURLValue = bundle.object(forInfoDictionaryKey: "TerranePremiumBaseURL")
    #endif
    premiumBaseURL = Self.configuredURL(premiumURLValue)
    googleClientID = Self.nonPlaceholder(bundle.object(forInfoDictionaryKey: "GIDClientID"))
    googleServerClientID = Self.nonPlaceholder(
      bundle.object(forInfoDictionaryKey: "GIDServerClientID")
    )
  }

  private static func configuredURL(_ raw: Any?) -> URL? {
    guard let value = nonPlaceholder(raw),
          let url = URL(string: value),
          (url.scheme == "https"
            || (url.scheme == "http"
              && ["127.0.0.1", "localhost", "::1"].contains(url.host ?? ""))),
          url.host != nil
    else {
      return nil
    }
    return url
  }

  private static func nonPlaceholder(_ raw: Any?) -> String? {
    guard let value = raw as? String else { return nil }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, !trimmed.contains("$(") else { return nil }
    return trimmed
  }
}
