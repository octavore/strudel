// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SafariExtension",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "CopyTabsApp",
            path: "Sources/CopyTabsApp"
        ),
        .executableTarget(
            name: "SafariExtensionHandler",
            path: "Sources/SafariExtensionHandler"
        ),
    ]
)
