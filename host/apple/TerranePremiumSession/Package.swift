// swift-tools-version: 5.9

import PackageDescription

let package = Package(
  name: "TerranePremiumSession",
  platforms: [
    .iOS(.v16),
    .macOS(.v13),
  ],
  products: [
    .library(name: "TerranePremiumSession", targets: ["TerranePremiumSession"])
  ],
  targets: [
    .target(
      name: "TerranePremiumSession",
      linkerSettings: [.linkedFramework("Security")]
    ),
    .testTarget(
      name: "TerranePremiumSessionTests",
      dependencies: ["TerranePremiumSession"]
    ),
  ]
)
