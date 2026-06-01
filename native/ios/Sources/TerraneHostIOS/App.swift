import SwiftUI

@main
struct TerraneHostIOSApp: App {
    var body: some Scene {
        WindowGroup {
            WebHostView()
                .ignoresSafeArea()
        }
    }
}
