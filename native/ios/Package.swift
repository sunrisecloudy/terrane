// swift-tools-version: 6.0

import PackageDescription

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
            linkerSettings: [
                .linkedLibrary("sqlite3")
            ]
        )
    ]
)
