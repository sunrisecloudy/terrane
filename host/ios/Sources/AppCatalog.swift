import Foundation

struct TerraneApp: Identifiable, Equatable {
  let id: String
  let name: String
  let icon: String
  let directory: URL
  let uiPath: String
}

enum BundledAppCatalog {
  static func load(bundle: Bundle = .main) -> [TerraneApp] {
    guard let appsRoot = bundle.resourceURL?.appendingPathComponent("apps", isDirectory: true),
          let directories = try? FileManager.default.contentsOfDirectory(
            at: appsRoot,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
          )
    else {
      return []
    }
    return directories.compactMap(parse).sorted {
      $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
    }
  }

  private static func parse(directory: URL) -> TerraneApp? {
    let manifestURL = directory.appendingPathComponent("manifest.json")
    guard let data = try? Data(contentsOf: manifestURL),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let id = object["id"] as? String,
          !id.isEmpty
    else {
      return nil
    }
    let uiPath = (object["ui"] as? String) ?? "index.html"
    guard !uiPath.isEmpty,
          FileManager.default.fileExists(atPath: directory.appendingPathComponent(uiPath).path)
    else {
      return nil
    }
    return TerraneApp(
      id: id,
      name: (object["name"] as? String) ?? id,
      icon: (object["icon"] as? String) ?? "square.grid.2x2",
      directory: directory.resolvingSymlinksInPath(),
      uiPath: uiPath
    )
  }
}
