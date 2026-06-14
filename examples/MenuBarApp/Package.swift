// swift-tools-version: 6.0
import PackageDescription

let package = Package(
  name: "Clipspect",
  platforms: [.macOS(.v14)],
  targets: [
    .executableTarget(
      name: "Clipspect",
      path: "Sources/Clipspect"
    )
  ]
)
