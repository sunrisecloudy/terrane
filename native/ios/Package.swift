// swift-tools-version: 6.0

import PackageDescription

let forgeFfiStaticLib = Context.environment["TERRANE_IOS_FORGE_FFI_STATICLIB"]
let forgeFfiLinkerSettings: [LinkerSetting] = [
    .linkedLibrary("sqlite3")
] + (forgeFfiStaticLib.map { forgeFfiStaticLib in
    [
        .unsafeFlags(
            ["-Xlinker", "-force_load", "-Xlinker", forgeFfiStaticLib],
            .when(platforms: [.iOS])
        )
    ]
} ?? [])

let package = Package(
    name: "TerraneHostIOS",
    platforms: [
        .iOS(.v17)
    ],
    products: [
        .executable(name: "TerraneHostIOS", targets: ["TerraneHostIOS"])
    ],
    targets: [
        .target(name: "CForgeCoreBridge"),
        .executableTarget(
            name: "TerraneHostIOS",
            dependencies: ["CForgeCoreBridge"],
            swiftSettings: [
                .define("DEBUG", .when(configuration: .debug))
            ],
            linkerSettings: forgeFfiLinkerSettings
        )
    ]
)
