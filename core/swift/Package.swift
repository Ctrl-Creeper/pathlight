// swift-tools-version: 5.9
// Local package that wraps the Rust core for the Xcode app target.
// `PathlightRustCoreFFI.xcframework` and the generated Swift file are produced
// by core/scripts/build-xcframework.sh.

import PackageDescription

let package = Package(
    name: "PathlightRustCore",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(name: "PathlightRustCore", targets: ["PathlightRustCore"])
    ],
    targets: [
        .binaryTarget(
            name: "PathlightRustCoreFFI",
            path: "PathlightRustCoreFFI.xcframework"
        ),
        .target(
            name: "PathlightRustCore",
            dependencies: ["PathlightRustCoreFFI"],
            path: "Sources/PathlightRustCore"
        )
    ]
)
