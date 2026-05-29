// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "NativeAIHostMac",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "NativeAIHostMac", targets: ["NativeAIHostMac"])
    ],
    targets: [
        .target(name: "CZigCoreBridge"),
        .executableTarget(
            name: "NativeAIHostMac",
            dependencies: ["CZigCoreBridge"],
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
            name: "NativeAIHostMacTests",
            dependencies: ["NativeAIHostMac"]
        )
    ]
)
