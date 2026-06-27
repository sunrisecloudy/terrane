import Foundation

struct TerraneApp: Equatable {
  let id: String
  let name: String
  let directory: URL
  let uiURL: URL
}

enum AppCatalog {
  static func discover(home: URL) -> [TerraneApp] {
    let fm = FileManager.default
    var appsById: [String: TerraneApp] = [:]

    for root in appRoots(home: home) {
      guard
        let dirs = try? fm.contentsOfDirectory(
          at: root,
          includingPropertiesForKeys: [.isDirectoryKey],
          options: [.skipsHiddenFiles]
        )
      else {
        continue
      }

      for dir in dirs where isDirectory(dir) {
        guard let app = readApp(in: dir, fm: fm), appsById[app.id] == nil else {
          continue
        }
        appsById[app.id] = app
      }
    }

    return appsById.values.sorted { lhs, rhs in
      let byName = lhs.name.localizedStandardCompare(rhs.name)
      if byName == .orderedSame {
        return lhs.id.localizedStandardCompare(rhs.id) == .orderedAscending
      }
      return byName == .orderedAscending
    }
  }

  private static func appRoots(home: URL) -> [URL] {
    let fm = FileManager.default
    var roots: [URL] = []

    if let repo = ProcessInfo.processInfo.environment["TERRANE_REPO"]?.trimmedNonEmpty {
      roots.append(URL(fileURLWithPath: repo).appendingPathComponent("apps"))
    }
    roots.append(URL(fileURLWithPath: fm.currentDirectoryPath).appendingPathComponent("apps"))
    roots.append(home.appendingPathComponent("apps"))
    if let resources = Bundle.main.resourceURL {
      roots.append(resources.appendingPathComponent("apps"))
    }

    var seen = Set<String>()
    return roots.compactMap { root in
      let path = root.standardizedFileURL.path
      guard seen.insert(path).inserted else { return nil }
      return root
    }
  }

  private static func isDirectory(_ url: URL) -> Bool {
    (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
  }

  private static func readApp(in dir: URL, fm: FileManager) -> TerraneApp? {
    let manifestURL = dir.appendingPathComponent("manifest.json")
    guard let data = try? Data(contentsOf: manifestURL),
      let object = try? JSONSerialization.jsonObject(with: data),
      let manifest = object as? [String: Any],
      let id = (manifest["id"] as? String)?.trimmedNonEmpty
    else {
      return nil
    }

    let name = (manifest["name"] as? String)?.trimmedNonEmpty ?? id
    let uiEntry = (manifest["ui"] as? String)?.trimmedNonEmpty ?? "index.html"
    guard let uiURL = resolveUI(uiEntry, in: dir, fm: fm) else {
      return nil
    }

    return TerraneApp(
      id: id,
      name: name,
      directory: dir.standardizedFileURL,
      uiURL: uiURL
    )
  }

  private static func resolveUI(_ entry: String, in dir: URL, fm: FileManager) -> URL? {
    guard !entry.hasPrefix("react:"),
      !entry.hasPrefix("/"),
      !entry.split(separator: "/").contains("..")
    else {
      return nil
    }

    let target = URL(fileURLWithPath: entry, relativeTo: dir).standardizedFileURL
    let basePath = dir.standardizedFileURL.path
    guard target.path == basePath || target.path.hasPrefix(basePath + "/") else {
      return nil
    }

    let ext = target.pathExtension.lowercased()
    guard ext == "html" || ext == "htm" else {
      return nil
    }

    var isDir: ObjCBool = false
    guard fm.fileExists(atPath: target.path, isDirectory: &isDir), !isDir.boolValue else {
      return nil
    }

    return target
  }
}

extension String {
  fileprivate var trimmedNonEmpty: String? {
    let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
  }
}
