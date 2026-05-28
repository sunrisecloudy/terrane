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
        .executableTarget(
            name: "NativeAIHostIOS",
            linkerSettings: [
                .linkedLibrary("sqlite3")
            ]
        )
    ]
)
