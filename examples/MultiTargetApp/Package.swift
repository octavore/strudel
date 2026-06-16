// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MultiTargetApp",
    platforms: [.macOS(.v14), .iOS(.v18)],
    targets: [
        .executableTarget(
            name: "MultiTargetApp",
            path: "Sources/MultiTargetApp"
        ),
        .executableTarget(
            name: "MultiTargetAppMobile",
            path: "Sources/MultiTargetAppMobile"
        ),
    ]
)
