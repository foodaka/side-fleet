// swift-tools-version: 6.0
import PackageDescription

// `SidefleetKit` holds everything that isn't drawing and builds on Linux too, so it can be tested
// anywhere. The app itself only builds on a Mac.
var products: [Product] = [.library(name: "SidefleetKit", targets: ["SidefleetKit"])]
var targets: [Target] = [
    .target(name: "SidefleetKit", dependencies: [.product(name: "Markdown", package: "swift-markdown")]),
    .testTarget(name: "SidefleetKitTests", dependencies: ["SidefleetKit"]),
]

#if os(macOS)
products.append(.executable(name: "Sidefleet", targets: ["Sidefleet"]))
targets.append(.executableTarget(
    name: "Sidefleet",
    dependencies: ["SidefleetKit", .product(name: "Sparkle", package: "Sparkle")],
    swiftSettings: [.swiftLanguageMode(.v5)]
))
#endif

let package = Package(
    name: "Sidefleet",
    platforms: [.macOS(.v14)],
    products: products,
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-markdown.git", from: "0.7.3"),
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.10.0"),
    ],
    targets: targets
)
