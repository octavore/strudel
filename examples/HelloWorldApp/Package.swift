// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "HelloWorld",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(name: "HelloWorld", path: "Sources")
    ]
)
