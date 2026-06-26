import Foundation

struct BundledAppEntry: Equatable, Decodable {
    let id: String
    let name: String
    let version: String
    let description: String
    let contentRating: ContentRating

    struct ContentRating: Equatable, Decodable {
        let minimumAge: Int
    }
}

struct MimeTypesConfig: Decodable {
    let extensions: [String: String]
    let `default`: String

    func mimeType(for pathExtension: String) -> String {
        let key = pathExtension.hasPrefix(".") ? pathExtension.lowercased() : ".\(pathExtension.lowercased())"
        return extensions[key] ?? `default`
    }
}

struct EnvVariablesConfig: Decodable {
    let portEnvVars: [String: String]
    let devControlEnvVars: [String: String]
    let signingKeyEnvVars: [String: String]
    let tokenFileEnvVars: [String: String]?
    let platformControlTokenEnvVars: [String: String]?
}

struct ControlPlaneConfig: Decodable {
    struct SigningKey: Decodable {
        let service: String
        let accountFormat: String
    }

    let signingKey: SigningKey
    let sessionIdPrefix: [String: String]
    let tokenFileLocation: String
}

struct RuntimeConfig: Decodable {
    let runtimeVersion: String
    let maxPackageBytes: Int
    let maxFileBytes: Int
    let defaultFileMaxBytes: Int
    let platform: String
    let target: String
}

struct EngineRoomTablesConfig: Decodable {
    let source: String
    let featureFlags: [String: Bool]
    let capabilities: [String]
    let tables: [String]
}

struct SnapshotTypesConfig: Decodable {
    let types: [String]
    let importOnly: [String]

    var creatableTypes: Set<String> { Set(types) }
}

struct AppStatusEnumsConfig: Decodable {
    let app_status: [String]
    let version_status: [String]
}

struct TrustLevelsConfig: Decodable {
    let levels: [String]
    let `default`: String
}

struct PackageManifestConfig: Decodable {
    let required: [String]
    let entry_point: String
    let allowed_files: [String]
    let allowed_prefixes: [String]
}

struct ControlCommandEntry: Decodable {
    let name: String
    let namespace: String
    let category: String
    let platforms: [String]
}

enum ForgeDataCatalogError: Error, Equatable {
    case missingResource(String)
    case decodeFailed(String)
}

/// Loads `forge/data/*.json` once at startup (repo checkout in dev, bundle resources when packaged).
final class ForgeDataCatalog: @unchecked Sendable {
    static let shared = ForgeDataCatalog()

    let bundledApps: [BundledAppEntry]
    let mimeTypes: MimeTypesConfig
    let envVariables: EnvVariablesConfig
    let controlPlaneConfig: ControlPlaneConfig
    let runtimeConfig: RuntimeConfig
    let engineRoomTables: EngineRoomTablesConfig
    let snapshotTypes: SnapshotTypesConfig
    let appStatusEnums: AppStatusEnumsConfig
    let trustLevels: TrustLevelsConfig
    let packageManifest: PackageManifestConfig
    let controlCommands: [ControlCommandEntry]
    private let macosControlTools: Set<String>

    init(loader: ((String) throws -> Data)? = nil) {
        let load = loader ?? Self.loadDataFile(named:)
        let loaded: LoadedCatalog
        do {
            loaded = try LoadedCatalog(load: load)
        } catch {
            fputs("ForgeDataCatalog failed to load: \(error)\n", stderr)
            fatalError("ForgeDataCatalog required data missing: \(error)")
        }
        bundledApps = loaded.bundledApps
        mimeTypes = loaded.mimeTypes
        envVariables = loaded.envVariables
        controlPlaneConfig = loaded.controlPlaneConfig
        runtimeConfig = loaded.runtimeConfig
        engineRoomTables = loaded.engineRoomTables
        snapshotTypes = loaded.snapshotTypes
        appStatusEnums = loaded.appStatusEnums
        trustLevels = loaded.trustLevels
        packageManifest = loaded.packageManifest
        controlCommands = loaded.controlCommands
        macosControlTools = Set(controlCommands.filter { $0.platforms.contains("macos") }.map(\.name))
    }

    private struct LoadedCatalog {
        let bundledApps: [BundledAppEntry]
        let mimeTypes: MimeTypesConfig
        let envVariables: EnvVariablesConfig
        let controlPlaneConfig: ControlPlaneConfig
        let runtimeConfig: RuntimeConfig
        let engineRoomTables: EngineRoomTablesConfig
        let snapshotTypes: SnapshotTypesConfig
        let appStatusEnums: AppStatusEnumsConfig
        let trustLevels: TrustLevelsConfig
        let packageManifest: PackageManifestConfig
        let controlCommands: [ControlCommandEntry]

        init(load: (String) throws -> Data) throws {
            bundledApps = try Self.decode([BundledAppEntry].self, from: try load("bundled-apps.json"))
            mimeTypes = try Self.decode(MimeTypesConfig.self, from: try load("mime-types.json"))
            envVariables = try Self.decode(EnvVariablesConfig.self, from: try load("env-variables.json"))
            controlPlaneConfig = try Self.decode(ControlPlaneConfig.self, from: try load("control-plane-config.json"))
            runtimeConfig = try Self.decode(RuntimeConfig.self, from: try load("runtime-config.json"))
            engineRoomTables = try Self.decode(EngineRoomTablesConfig.self, from: try load("engine-room-tables.json"))
            snapshotTypes = try Self.decode(SnapshotTypesConfig.self, from: try load("snapshot-types.json"))
            appStatusEnums = try Self.decode(AppStatusEnumsConfig.self, from: try load("app-status-enums.json"))
            trustLevels = try Self.decode(TrustLevelsConfig.self, from: try load("trust-levels.json"))
            packageManifest = try Self.decode(PackageManifestConfig.self, from: try load("package-manifest.json"))
            controlCommands = try Self.decode([ControlCommandEntry].self, from: try load("control-commands.json"))
        }

        private static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
            do {
                return try JSONDecoder().decode(type, from: data)
            } catch {
                throw ForgeDataCatalogError.decodeFailed(String(describing: error))
            }
        }
    }

    var defaultTrustLevel: String { trustLevels.default }
    var runtimeVersion: String { runtimeConfig.runtimeVersion }
    var macosPortEnvVar: String { envVariables.portEnvVars["macos"] ?? "TERRANE_MACOS_CONTROL_PORT" }
    var macosDevControlEnvVar: String { envVariables.devControlEnvVars["macos"] ?? "TERRANE_MACOS_DEV_CONTROL" }
    var macosSigningKeyEnvVar: String { envVariables.signingKeyEnvVars["macos"] ?? "TERRANE_MACOS_SIGNING_KEY_ACCOUNT" }
    var macosSigningKeyService: String { controlPlaneConfig.signingKey.service }
    var macosSigningKeyAccount: String {
        controlPlaneConfig.signingKey.accountFormat.replacingOccurrences(of: "{platform}", with: "macos")
    }
    var macosSessionIdPrefix: String { controlPlaneConfig.sessionIdPrefix["macos"] ?? "control_" }

    func isKnownControlTool(_ name: String) -> Bool {
        macosControlTools.contains(name)
    }

    func isAllowedSnapshotType(_ type: String) -> Bool {
        snapshotTypes.creatableTypes.contains(type)
    }

    static func dataDirectoryURL() -> URL? {
        if let resourceRoot = Bundle.main.resourceURL {
            for candidate in ["forge-data", "forge/data", "data"] {
                let bundled = resourceRoot.appendingPathComponent(candidate)
                if FileManager.default.fileExists(atPath: bundled.path) {
                    return bundled
                }
            }
        }
        let repo = RuntimeResourceLocator.repoRootURL().appendingPathComponent("forge/data")
        return FileManager.default.fileExists(atPath: repo.path) ? repo : nil
    }

    static func loadDataFile(named filename: String) throws -> Data {
        if let directory = dataDirectoryURL() {
            let url = directory.appendingPathComponent(filename)
            if FileManager.default.fileExists(atPath: url.path) {
                return try Data(contentsOf: url)
            }
        }
        throw ForgeDataCatalogError.missingResource(filename)
    }

    private static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw ForgeDataCatalogError.decodeFailed(String(describing: error))
        }
    }
}