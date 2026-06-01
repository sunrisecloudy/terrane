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
        .target(name: "CZigCoreBridge"),
        .executableTarget(
            name: "TerraneHostIOS",
            dependencies: ["CZigCoreBridge"],
            swiftSettings: [
                .define("DEBUG", .when(configuration: .debug))
            ],
            linkerSettings: [
                .linkedLibrary("sqlite3")
            ]
        )
    ]
)
