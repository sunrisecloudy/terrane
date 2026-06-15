// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "TerraneHostMac",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "TerraneHostMac", targets: ["TerraneHostMac"])
    ],
    targets: [
        .target(name: "CForgeCoreBridge"),
        .executableTarget(
            name: "TerraneHostMac",
            dependencies: ["CForgeCoreBridge"],
            swiftSettings: [
                .define("DEBUG", .when(configuration: .debug))
            ],
            linkerSettings: [
                .linkedFramework("Network"),
                .linkedFramework("Security"),
                .linkedLibrary("sqlite3")
            ]
        ),
        .testTarget(
            name: "TerraneHostMacTests",
            dependencies: ["TerraneHostMac"]
        )
    ]
)
