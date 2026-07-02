import Foundation

/// The shared terrane-host landing page, rendered over the C ABI for the
/// natively discovered catalog. Cards link through the terrane-app scheme;
/// the navigation delegate routes those clicks back through native selection
/// so the bridge stays scoped to the selected app.
enum HomePage {
  static let appHrefTemplate = "\(AppSchemeHandler.scheme)://{id}/frame/"

  static func render(apps: [TerraneApp]) -> String? {
    let catalog: [String: Any] = [
      "apps": apps.map { ["id": $0.id, "name": $0.name, "has_ui": true] }
    ]
    guard
      let data = try? JSONSerialization.data(
        withJSONObject: catalog,
        options: [.sortedKeys, .withoutEscapingSlashes]
      ),
      let json = String(data: data, encoding: .utf8)
    else {
      return nil
    }

    return json.withCString { catalogC -> String? in
      appHrefTemplate.withCString { templateC -> String? in
        var out: UnsafeMutablePointer<CChar>?
        var err: UnsafeMutablePointer<CChar>?
        let rc = terrane_home_page(catalogC, templateC, &out, &err)
        defer {
          if let o = out { terrane_string_free(o) }
          if let e = err { terrane_string_free(e) }
        }
        guard rc == 0, let o = out else {
          return nil
        }
        return String(cString: o)
      }
    }
  }

  /// The app id a home-page card click navigates to, if `url` is an app frame
  /// root (`terrane-app://<id>/frame/`). Asset sub-paths are not card links.
  static func appId(for url: URL) -> String? {
    guard url.scheme == AppSchemeHandler.scheme,
      url.path == "/frame" || url.path == "/frame/"
    else {
      return nil
    }
    return url.host?.removingPercentEncoding
  }
}
