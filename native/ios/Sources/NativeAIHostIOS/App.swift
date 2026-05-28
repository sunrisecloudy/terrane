import SwiftUI

@main
struct NativeAIHostIOSApp: App {
    var body: some Scene {
        WindowGroup {
            WebHostView()
                .ignoresSafeArea()
        }
    }
}
