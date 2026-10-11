// swift-tools-version:5.5
import PackageDescription

let package = Package(
    name: "tauri-plugin-finance-sync",
    platforms: [.iOS(.v15)],
    products: [.library(name: "tauri-plugin-finance-sync", type: .static,
                        targets: ["tauri-plugin-finance-sync"])],
    dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")],
    targets: [.target(name: "tauri-plugin-finance-sync", dependencies: ["Tauri"], path: "Sources")]
)
