import SwiftUI
import UIKit
import WebKit

#if DEBUG
import Darwin
#endif

struct WebHostView: UIViewRepresentable {
    func makeUIView(context: Context) -> WKWebView {
        let bridge = context.coordinator.bridge
        let contentController = WKUserContentController()
        contentController.addScriptMessageHandler(bridge, contentWorld: .page, name: "NativeAIPlatformBridge")

        let configuration = WKWebViewConfiguration()
        configuration.userContentController = contentController
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(RuntimeSchemeHandler(), forURLScheme: RuntimeResourceLocator.scheme)

        let webView = WKWebView(frame: .zero, configuration: configuration)
        bridge.setDialogPresenterProvider { [weak webView] in
            guard let webView else { return nil }
            return Self.presentingViewController(from: webView)
        }
#if DEBUG
        if let smokeProbe = context.coordinator.smokeProbe {
            webView.navigationDelegate = smokeProbe
        }
#endif
        webView.load(URLRequest(url: RuntimeResourceLocator.runtimeIndexURL()))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    @MainActor
    final class Coordinator {
        let bridge = WebBridge()
#if DEBUG
        let smokeProbe = IOSSmokeRuntimeProbe.fromCommandLine()
#endif
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

#if DEBUG
final class IOSSmokeRuntimeProbe: NSObject, WKNavigationDelegate {
    static let loadedMarker = "NATIVE_AI_IOS_SMOKE_RUNTIME_LOADED"
    static let markerFileName = "native-ai-ios-smoke-runtime-loaded.txt"

    private let exitAfterLoad: Bool

    private init(exitAfterLoad: Bool) {
        self.exitAfterLoad = exitAfterLoad
    }

    static func fromCommandLine() -> IOSSmokeRuntimeProbe? {
        let args = CommandLine.arguments
        guard args.contains("--native-ai-smoke-runtime-load") ||
            args.contains("--native-ai-smoke-exit-on-runtime-load")
        else {
            return nil
        }
        return IOSSmokeRuntimeProbe(exitAfterLoad: args.contains("--native-ai-smoke-exit-on-runtime-load"))
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard webView.url == RuntimeResourceLocator.runtimeIndexURL() else { return }
        emitSmokeMarker(Self.loadedMarker)
        if exitAfterLoad {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
                Darwin.exit(0)
            }
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        emitSmokeMarker("NATIVE_AI_IOS_SMOKE_RUNTIME_FAILED: \(error.localizedDescription)")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        emitSmokeMarker("NATIVE_AI_IOS_SMOKE_RUNTIME_FAILED: \(error.localizedDescription)")
    }

    private func emitSmokeMarker(_ marker: String) {
        print(marker)
        fflush(stdout)
        let markerURL = FileManager.default.temporaryDirectory.appendingPathComponent(Self.markerFileName)
        try? marker.write(to: markerURL, atomically: true, encoding: .utf8)
    }
}
#endif

final class RuntimeSchemeHandler: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard let requestURL = urlSchemeTask.request.url,
              let fileURL = RuntimeResourceLocator.fileURL(forRuntimeURL: requestURL)
        else {
            urlSchemeTask.didFailWithError(RuntimeResourceError.notFound)
            return
        }

        do {
            let data = try Data(contentsOf: fileURL)
            let response = HTTPURLResponse(
                url: requestURL,
                statusCode: 200,
                httpVersion: nil,
                headerFields: [
                    "Content-Type": "\(RuntimeResourceLocator.mimeType(for: fileURL)); charset=utf-8",
                    "Content-Length": "\(data.count)"
                ]
            )!
            urlSchemeTask.didReceive(response)
            urlSchemeTask.didReceive(data)
            urlSchemeTask.didFinish()
        } catch {
            urlSchemeTask.didFailWithError(error)
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {}
}

enum RuntimeResourceLocator {
    static let scheme = "app-runtime"

    static func runtimeIndexURL() -> URL {
        URL(string: "\(scheme)://runtime/index.html")!
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

    static func fileURL(forRuntimeURL url: URL) -> URL? {
        guard url.scheme == scheme else { return nil }
        let logicalPath = logicalResourcePath(for: url)
        guard isAllowedLogicalPath(logicalPath) else { return nil }

        if logicalPath.hasPrefix("runtime/") {
            let relative = String(logicalPath.dropFirst("runtime/".count))
            return firstExistingURL([
                Bundle.main.resourceURL?.appendingPathComponent("runtime").appendingPathComponent(relative),
                repoRootURL().appendingPathComponent("runtime-web").appendingPathComponent(relative)
            ])
        }

        if logicalPath.hasPrefix("webapps/examples/") {
            return firstExistingURL([
                Bundle.main.resourceURL?.appendingPathComponent(logicalPath),
                repoRootURL().appendingPathComponent(logicalPath)
            ])
        }

        return nil
    }

    static func mimeType(for fileURL: URL) -> String {
        switch fileURL.pathExtension.lowercased() {
        case "html":
            return "text/html"
        case "css":
            return "text/css"
        case "js":
            return "text/javascript"
        case "json":
            return "application/json"
        default:
            return "text/plain"
        }
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

    private static func logicalResourcePath(for url: URL) -> String {
        let path = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if url.host == "runtime", path == "index.html" {
            return "runtime/index.html"
        }
        return path
    }

    private static func isAllowedLogicalPath(_ path: String) -> Bool {
        !path.isEmpty &&
            !path.contains("..") &&
            !path.contains("\\") &&
            (path.hasPrefix("runtime/") || path.hasPrefix("webapps/examples/"))
    }

    private static func firstExistingURL(_ urls: [URL?]) -> URL? {
        urls.compactMap { $0 }.first { FileManager.default.fileExists(atPath: $0.path) }
    }
}

enum RuntimeResourceError {
    static let notFound = NSError(
        domain: NSURLErrorDomain,
        code: NSURLErrorFileDoesNotExist,
        userInfo: [NSLocalizedDescriptionKey: "Runtime resource was not found"]
    )
}
