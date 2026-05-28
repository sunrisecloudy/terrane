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
        .executableTarget(
            name: "NativeAIHostMac",
            linkerSettings: [
                .linkedLibrary("sqlite3")
            ]
        )
    ]
)
