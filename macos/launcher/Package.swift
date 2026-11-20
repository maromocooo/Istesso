// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "istesso-launcher",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "LauncherCore", targets: ["LauncherCore"]),
        .library(name: "LauncherUI", targets: ["LauncherUI"]),
        .executable(name: "istesso-launcher", targets: ["istesso-launcher"]),
        .executable(name: "launcher-check", targets: ["launcher-check"]),
        .executable(name: "launcher-render", targets: ["launcher-render"]),
    ],
    targets: [
        .target(name: "LauncherCore"),
        .target(name: "LauncherUI", dependencies: ["LauncherCore"]),
        .executableTarget(name: "istesso-launcher", dependencies: ["LauncherUI"]),
        .executableTarget(name: "launcher-check", dependencies: ["LauncherCore"]),
        .executableTarget(name: "launcher-render", dependencies: ["LauncherUI"]),
    ],
    swiftLanguageVersions: [.v5]
)
