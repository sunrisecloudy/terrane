import Foundation

struct AppConfiguration {
  let premiumBaseURL: URL?
  let googleClientID: String?
  let googleServerClientID: String?

  static let current = AppConfiguration(bundle: .main)

  init(bundle: Bundle) {
    premiumBaseURL = Self.configuredURL(bundle.object(forInfoDictionaryKey: "TerranePremiumBaseURL"))
    googleClientID = Self.nonPlaceholder(bundle.object(forInfoDictionaryKey: "GIDClientID"))
    googleServerClientID = Self.nonPlaceholder(
      bundle.object(forInfoDictionaryKey: "GIDServerClientID")
    )
  }

  private static func configuredURL(_ raw: Any?) -> URL? {
    guard let value = nonPlaceholder(raw),
          let url = URL(string: value),
          url.scheme == "https",
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
