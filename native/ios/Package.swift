// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "NativeAIHostIOS",
    platforms: [
        .iOS(.v17)
    ],
    products: [
        .executable(name: "NativeAIHostIOS", targets: ["NativeAIHostIOS"])
    ],
    targets: [
        .target(name: "CZigCoreBridge"),
        .executableTarget(
            name: "NativeAIHostIOS",
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
