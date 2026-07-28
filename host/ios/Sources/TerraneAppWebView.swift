import SwiftUI
import UIKit
import WebKit

struct TerraneAppWebView: UIViewRepresentable {
  let app: TerraneApp
  let runtime: any TerraneRuntime

  func makeCoordinator() -> Coordinator {
    Coordinator(app: app, runtime: runtime)
  }

  func makeUIView(context: Context) -> WKWebView {
    let content = WKUserContentController()
    content.addUserScript(
      WKUserScript(
        source: Self.bridgeScript,
        injectionTime: .atDocumentStart,
        forMainFrameOnly: false
      )
    )
    content.addScriptMessageHandler(
      context.coordinator,
      contentWorld: .page,
      name: "terrane"
    )
    let configuration = WKWebViewConfiguration()
    configuration.websiteDataStore = .nonPersistent()
    configuration.userContentController = content
    configuration.setURLSchemeHandler(
      AppResourceSchemeHandler(app: app),
      forURLScheme: "terrane-app"
    )
    let webView = WKWebView(frame: .zero, configuration: configuration)
    webView.navigationDelegate = context.coordinator
    webView.isInspectable = false
    webView.scrollView.contentInsetAdjustmentBehavior = .always
    webView.load(URLRequest(url: context.coordinator.frameURL))
    return webView
  }

  func updateUIView(_ webView: WKWebView, context: Context) {}

  static let bridgeScript = """
    (() => {
      "use strict";
      const invoke = (verb, ...args) =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "invoke",
          verb: String(verb),
          args: args.map((value) =>
            typeof value === "string" ? value : JSON.stringify(value)
          )
        });
      Object.defineProperty(window, "terrane", {
        value: Object.freeze({ invoke }),
        enumerable: true,
        configurable: false,
        writable: false
      });
    })();
    """

  final class Coordinator: NSObject, WKScriptMessageHandlerWithReply, WKNavigationDelegate {
    let app: TerraneApp
    let runtime: any TerraneRuntime
    let frameURL: URL

    init(app: TerraneApp, runtime: any TerraneRuntime) {
      self.app = app
      self.runtime = runtime
      var components = URLComponents()
      components.scheme = "terrane-app"
      components.host = app.id
      components.path = "/frame/\(app.uiPath)"
      frameURL = components.url!
    }

    func userContentController(
      _ userContentController: WKUserContentController,
      didReceive message: WKScriptMessage,
      replyHandler: @escaping (Any?, String?) -> Void
    ) {
      guard message.frameInfo.securityOrigin.protocol == "terrane-app",
            message.frameInfo.securityOrigin.host == app.id,
            let body = message.body as? [String: Any],
            body["kind"] as? String == "invoke",
            let verb = body["verb"] as? String,
            !verb.isEmpty
      else {
        replyHandler(nil, "terrane: rejected untrusted bridge message")
        return
      }
      let arguments = (body["args"] as? [Any] ?? []).map {
        ($0 as? String) ?? String(describing: $0)
      }
      Task {
        do {
          let result = try await runtime.invoke(
            appID: app.id,
            verb: verb,
            arguments: arguments
          )
          replyHandler(result, nil)
        } catch {
          replyHandler(nil, error.localizedDescription)
        }
      }
    }

    func webView(
      _ webView: WKWebView,
      decidePolicyFor navigationAction: WKNavigationAction,
      decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
      guard let url = navigationAction.request.url else {
        decisionHandler(.cancel)
        return
      }
      if url.scheme == "terrane-app", url.host == app.id {
        decisionHandler(.allow)
      } else if navigationAction.targetFrame?.isMainFrame == true,
                ["https", "http"].contains(url.scheme?.lowercased() ?? "")
      {
        decisionHandler(.cancel)
        UIApplication.shared.open(url)
      } else {
        decisionHandler(.cancel)
      }
    }
  }
}

final class AppResourceSchemeHandler: NSObject, WKURLSchemeHandler {
  private let app: TerraneApp

  init(app: TerraneApp) {
    self.app = app
  }

  func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
    guard let url = urlSchemeTask.request.url,
          url.scheme == "terrane-app",
          url.host == app.id
    else {
      fail(urlSchemeTask, code: 403)
      return
    }
    let prefix = "/frame/"
    guard url.path.hasPrefix(prefix) else {
      fail(urlSchemeTask, code: 404)
      return
    }
    let relative = String(url.path.dropFirst(prefix.count)).removingPercentEncoding ?? ""
    let candidate = app.directory.appendingPathComponent(relative).standardizedFileURL
    let root = app.directory.standardizedFileURL.path + "/"
    guard candidate.path.hasPrefix(root),
          !candidate.hasDirectoryPath,
          let data = try? Data(contentsOf: candidate, options: [.mappedIfSafe])
    else {
      fail(urlSchemeTask, code: 404)
      return
    }
    let response = HTTPURLResponse(
      url: url,
      statusCode: 200,
      httpVersion: "HTTP/1.1",
      headerFields: [
        "Content-Type": Self.contentType(candidate.pathExtension),
        "Cache-Control": "no-store",
        "Content-Security-Policy":
          "default-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
        "X-Content-Type-Options": "nosniff",
      ]
    )!
    urlSchemeTask.didReceive(response)
    urlSchemeTask.didReceive(data)
    urlSchemeTask.didFinish()
  }

  func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {}

  private func fail(_ task: WKURLSchemeTask, code: Int) {
    task.didFailWithError(
      NSError(
        domain: NSURLErrorDomain,
        code: code == 403 ? NSURLErrorNoPermissionsToReadFile : NSURLErrorFileDoesNotExist
      )
    )
  }

  private static func contentType(_ pathExtension: String) -> String {
    switch pathExtension.lowercased() {
    case "html": return "text/html; charset=utf-8"
    case "js", "mjs": return "text/javascript; charset=utf-8"
    case "css": return "text/css; charset=utf-8"
    case "json": return "application/json; charset=utf-8"
    case "svg": return "image/svg+xml"
    case "png": return "image/png"
    case "jpg", "jpeg": return "image/jpeg"
    case "webp": return "image/webp"
    case "woff2": return "font/woff2"
    default: return "application/octet-stream"
    }
  }
}
