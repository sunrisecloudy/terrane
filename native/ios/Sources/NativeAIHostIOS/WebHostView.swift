import SwiftUI
import UIKit
import WebKit

struct WebHostView: UIViewRepresentable {
    func makeUIView(context: Context) -> WKWebView {
        let bridge = context.coordinator.bridge
        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()

        let webView = WKWebView(frame: .zero, configuration: configuration)
        bridge.setDialogPresenterProvider { [weak webView] in
            guard let webView else { return nil }
            return Self.presentingViewController(from: webView)
        }
        webView.loadFileURL(RuntimeResourceLocator.runtimeIndexURL(), allowingReadAccessTo: RuntimeResourceLocator.runtimeReadAccessURL())
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    @MainActor
    final class Coordinator {
        let bridge = WebBridge()
    }

    @MainActor
    private static func presentingViewController(from view: UIView) -> UIViewController? {
        var responder: UIResponder? = view
        while let current = responder {
            if let viewController = current as? UIViewController {
                return viewController
            }
            responder = current.next
        }
        return view.window?.rootViewController
    }
}

enum RuntimeResourceLocator {
    static func runtimeIndexURL() -> URL {
        if let bundled = Bundle.main.url(forResource: "index", withExtension: "html", subdirectory: "runtime") {
            return bundled
        }
        return repoRootURL().appendingPathComponent("runtime-web/index.html")
    }

    static func runtimeReadAccessURL() -> URL {
        if let bundledRuntime = Bundle.main.resourceURL {
            return bundledRuntime
        }
        return repoRootURL()
    }

    static func exampleManifestURL(for appId: String) -> URL? {
        if let bundled = Bundle.main.url(forResource: "manifest", withExtension: "json", subdirectory: "examples/\(appId)") {
            return bundled
        }
        return repoRootURL()
            .appendingPathComponent("webapps/examples")
            .appendingPathComponent(appId)
            .appendingPathComponent("manifest.json")
    }

    static func repoRootURL() -> URL {
        var url = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<4 {
            if FileManager.default.fileExists(atPath: url.appendingPathComponent("docs/00_PRD.md").path) {
                return url
            }
            url.deleteLastPathComponent()
        }
        return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    }
}
