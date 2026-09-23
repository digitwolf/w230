// swift-tools-version: 6.0
// xtool SwiftPM project (Linux-hosted iOS build): one library product = the app.
// `scripts/deploy.sh` builds, signs and installs it on a USB-connected iPhone;
// `project.yml` (XcodeGen) remains for people on a Mac.

import PackageDescription

let package = Package(
    name: "W230",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(name: "W230", targets: ["W230"]),
    ],
    targets: [
        .target(
            name: "W230",
            swiftSettings: [
                // Swift 5 language mode: CoreBluetooth's delegate types are not
                // Sendable and the code hops to the main actor explicitly.
                .swiftLanguageMode(.v5),
            ]
        ),
    ]
)
