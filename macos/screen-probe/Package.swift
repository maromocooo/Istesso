// swift-tools-version:5.9
import PackageDescription

// Measurement tool for the Mac client: watches two windows on the screen and records when each
// one reacted to the same input. See docs/adr/0008-screen-recording-for-measurement.md.
let package = Package(
    name: "istesso-screen-probe",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "istesso-screen-probe",
            path: "Sources/istesso-screen-probe"
        )
    ]
)
