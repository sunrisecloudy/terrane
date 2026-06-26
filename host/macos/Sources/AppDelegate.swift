import AppKit
import WebKit

/// The macOS host window: one app's UI in a WKWebView. This skeleton loads the
/// app's index.html statically (the dev fallback in the page keeps it
/// interactive); the FFI bridge is wired in the next commit.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!
    private var webView: WKWebView!

    func applicationDidFinishLaunching(_ notification: Notification) {
        let appId = Self.parseAppId()
        guard let appDir = Self.resolveAppDir(appId) else {
            fatalError("terrane-host: no app dir with index.html for \(appId)")
        }
        let indexURL = appDir.appendingPathComponent("index.html")

        webView = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())

        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 420, height: 560),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = appId.capitalized
        window.center()
        window.contentView = webView

        // allowingReadAccessTo the whole app dir so the page can load siblings later.
        webView.loadFileURL(indexURL, allowingReadAccessTo: appDir)

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    /// App id from `open Terrane.app --args <id>` / argv[1], default "todo".
    static func parseAppId() -> String {
        let args = CommandLine.arguments
        if args.count > 1, !args[1].hasPrefix("-") {
            return args[1]
        }
        return "todo"
    }

    /// Find apps/<id> with an index.html: $TERRANE_REPO/apps, cwd/apps, or the
    /// bundle's Resources/apps.
    static func resolveAppDir(_ appId: String) -> URL? {
        let fm = FileManager.default
        var candidates: [URL] = []
        if let repo = ProcessInfo.processInfo.environment["TERRANE_REPO"] {
            candidates.append(URL(fileURLWithPath: repo).appendingPathComponent("apps/\(appId)"))
        }
        candidates.append(
            URL(fileURLWithPath: fm.currentDirectoryPath).appendingPathComponent("apps/\(appId)")
        )
        if let resources = Bundle.main.resourceURL {
            candidates.append(resources.appendingPathComponent("apps/\(appId)"))
        }
        return candidates.first {
            fm.fileExists(atPath: $0.appendingPathComponent("index.html").path)
        }
    }
}
