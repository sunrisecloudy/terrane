import Foundation

struct TerraneApp: Identifiable, Equatable {
  let id: String
  let name: String
  let icon: String
  let directory: URL
  let uiPath: String
  let runtime: String
  let interfaces: [String]

  var registrationArguments: [String] {
    var arguments = [
      id,
      name,
      "--source",
      directory.path,
      "--runtime",
      runtime,
    ]
    if !interfaces.isEmpty {
      arguments.append(contentsOf: ["--interfaces", interfaces.joined(separator: ",")])
    }
    arguments.append("--refresh-source")
    return arguments
  }
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
    let runtime =
      ((object["runtime"] as? String) ?? "js")
      .trimmingCharacters(in: .whitespacesAndNewlines)
    let interfaces =
      (object["interfaces"] as? [String] ?? [])
      .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
    return TerraneApp(
      id: id,
      name: (object["name"] as? String) ?? id,
      icon: (object["icon"] as? String) ?? "square.grid.2x2",
      directory: directory.resolvingSymlinksInPath(),
      uiPath: uiPath,
      runtime: runtime.isEmpty ? "js" : runtime,
      interfaces: interfaces
    )
  }
}

enum AppPinPreferences {
  static let storageKey = "terrane.pinned-app-ids"

  static func load(from defaults: UserDefaults) -> Set<String> {
    Set(
      (defaults.stringArray(forKey: storageKey) ?? [])
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { !$0.isEmpty }
    )
  }

  static func save(_ appIDs: Set<String>, to defaults: UserDefaults) {
    defaults.set(appIDs.sorted(), forKey: storageKey)
  }

  static func ordered(_ apps: [TerraneApp], pinnedAppIDs: Set<String>) -> [TerraneApp] {
    apps.sorted { left, right in
      let leftPinned = pinnedAppIDs.contains(left.id)
      let rightPinned = pinnedAppIDs.contains(right.id)
      if leftPinned != rightPinned {
        return leftPinned
      }
      let nameOrder = left.name.localizedCaseInsensitiveCompare(right.name)
      if nameOrder != .orderedSame {
        return nameOrder == .orderedAscending
      }
      return left.id < right.id
    }
  }
}

enum NativeAppIconCatalog {
  static let symbolByAppID: [String: String] = [
    "app-builder": "pencil.and.ruler",
    "bmi-calculator": "gauge.with.dots.needle.50percent",
    "chat": "bubble.left.and.bubble.right",
    "control-room": "server.rack",
    "health": "heart.text.square",
    "os-monitor": "waveform.path.ecg.rectangle",
    "password-manager": "lock.shield",
    "photobooth": "camera",
    "pixel-paint": "paintpalette",
    "scribe": "mic",
    "search-notes": "doc.text.magnifyingglass",
    "spending": "creditcard",
    "todo": "checklist",
    "todo-cli": "terminal",
    "todo-cli-collaborate": "person.2",
    "tomorrow": "calendar.badge.clock",
    "visual-intake": "camera.viewfinder",
  ]

  static func systemName(for app: TerraneApp) -> String {
    symbolByAppID[app.id] ?? "app.dashed"
  }
}
