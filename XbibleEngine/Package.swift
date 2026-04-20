// swift-tools-version: 6.3
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "XbibleEngine",
    products: [
        // Products define the executables and libraries a package produces, making them visible to other packages.
        .library(
            name: "XbibleEngine",
            targets: ["XbibleEngine"]
        ),
    ],
    targets: [
        // Targets are the basic building blocks of a package, defining a module or a test suite.
        // Targets can depend on other targets in this package and products from dependencies.
        .target(
            name: "XbibleEngine",
            dependencies: ["xbible_engineFFI"]
        ),
        .binaryTarget(
            name: "xbible_engineFFI",
            path: "xbible_engine.xcframework"
        ),
        .testTarget(
            name: "XbibleEngineTests",
            dependencies: ["XbibleEngine"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
