import Foundation

enum BundledAppCatalog {
    private static let bundledAppIds = [
        "notes-lite",
        "task-workbench",
        "file-transformer",
        "api-dashboard",
        "core-replay-lab"
    ]

    static func appIndexData() -> Data {
        let maximumAge = maximumAllowedAge()
        let records = bundledAppIds.compactMap { record(for: $0) }
            .filter { record in
                guard let maximumAge else { return true }
                return record.minimumAge <= maximumAge
            }
            .map(\.json)
        var body: [String: Any] = [
            "source": "ios-bundled",
            "apps": records
        ]
        if let maximumAge {
            body["maximumAllowedAge"] = maximumAge
        }
        return (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])) ?? Data(#"{"source":"ios-bundled","apps":[]}"#.utf8)
    }

    static func isAllowed(appId: String) -> Bool {
        denialReason(appId: appId) == nil
    }

    static func denialReason(appId: String) -> String? {
        guard let record = record(for: appId) else {
            return "not_bundled"
        }
        guard let maximumAge = maximumAllowedAge() else {
            return nil
        }
        return record.minimumAge <= maximumAge ? nil : "content_rating"
    }

    static func maximumAllowedAge() -> Int? {
        if let age = commandLineValue(after: "--native-ai-max-content-age") {
            return age
        }
        if let raw = ProcessInfo.processInfo.environment["NATIVE_AI_IOS_MAX_CONTENT_AGE"],
           let age = Int(raw) {
            return age
        }
        return nil
    }

    private static func record(for appId: String) -> BundledAppRecord? {
        guard bundledAppIds.contains(appId),
              let manifestURL = RuntimeResourceLocator.exampleManifestURL(for: appId),
              let data = try? Data(contentsOf: manifestURL),
              let manifest = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = manifest["id"] as? String,
              let name = manifest["name"] as? String,
              let version = manifest["version"] as? String,
              let description = manifest["description"] as? String,
              let contentRating = manifest["contentRating"] as? [String: Any],
              let minimumAge = contentRating["minimumAge"] as? Int
        else {
            return nil
        }
        return BundledAppRecord(
            id: id,
            title: name,
            description: description,
            version: version,
            contentRating: contentRating,
            minimumAge: minimumAge
        )
    }

    private static func commandLineValue(after name: String) -> Int? {
        let args = CommandLine.arguments
        guard let index = args.firstIndex(of: name),
              args.indices.contains(args.index(after: index))
        else {
            return nil
        }
        return Int(args[args.index(after: index)])
    }
}

private struct BundledAppRecord {
    let id: String
    let title: String
    let description: String
    let version: String
    let contentRating: [String: Any]
    let minimumAge: Int

    var json: [String: Any] {
        [
            "id": id,
            "name": title,
            "title": title,
            "description": description,
            "version": version,
            "contentRating": contentRating
        ]
    }
}
