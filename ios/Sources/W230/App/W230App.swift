import SwiftUI

/// Owns the single `DeviceSession` for the app's lifetime. `@State` is not
/// available in this toolchain (macOS-only macro plugin), so the session is
/// held by a `@StateObject` wrapper, which gives the same once-per-scene
/// ownership; views still read it through `@Environment(DeviceSession.self)`.
@MainActor
final class SessionHolder: ObservableObject {
    let session = DeviceSession()
}

@main
struct W230App: App {
    @StateObject private var holder = SessionHolder()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(holder.session)
        }
    }
}
