import SwiftUI

@main
struct W230App: App {
    @State private var session = DeviceSession()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(session)
        }
    }
}
