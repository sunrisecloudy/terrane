import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var hostView: WebHostView?
#if DEBUG
    private var controlPlane: DevControlPlane?
#endif

    func applicationDidFinishLaunching(_ notification: Notification) {
#if DEBUG
        if MacSmokeProbe.emitLaunchMarkerAndExitIfRequested() {
            return
        }
        do {
            if let controlPlane = try DevControlPlane.enabledFromProcess() {
                try controlPlane.start()
                self.controlPlane = controlPlane
                if let port = controlPlane.boundPort {
                    print("NATIVE_AI_MACOS_CONTROL_READY port=\(port)")
                }
            }
        } catch {
            fputs("macOS dev control plane failed to start: \(error)\n", stderr)
        }
#endif

        let hostView = WebHostView()
        self.hostView = hostView

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1200, height: 820),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Native AI Webapp Platform"
        window.center()
        window.contentView = hostView
        window.makeKeyAndOrderFront(nil)
        self.window = window
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

#if DEBUG
enum MacSmokeProbe {
    static let launchedMarker = "NATIVE_AI_MACOS_SMOKE_APP_LAUNCHED"
    static let markerFileName = "native-ai-macos-smoke-launched.txt"

    static func emitLaunchMarkerAndExitIfRequested() -> Bool {
        let args = CommandLine.arguments
        guard args.contains("--native-ai-smoke-launch") else { return false }
        print(launchedMarker)
        fflush(stdout)
        let markerURL = smokeMarkerURL()
        try? FileManager.default.createDirectory(
            at: markerURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? launchedMarker.write(to: markerURL, atomically: true, encoding: .utf8)
        if args.contains("--native-ai-smoke-exit-after-launch") {
            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
            return true
        }
        return false
    }

    private static func smokeMarkerURL() -> URL {
        if let overridePath = ProcessInfo.processInfo.environment["NATIVE_AI_MACOS_SMOKE_MARKER_PATH"],
           !overridePath.isEmpty {
            return URL(fileURLWithPath: overridePath)
        }
        return URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(markerFileName)
    }
}
#endif
