import SwiftUI
import UIKit
import WebKit

struct TerraneAppWebView: UIViewRepresentable {
  let app: TerraneApp
  let runtime: any TerraneRuntime
  let healthAutoSync: IOSHealthAutoSync?
  let permissionHandler: (TerraneApp, [String]) async -> Bool

  func makeCoordinator() -> Coordinator {
    Coordinator(
      app: app,
      runtime: runtime,
      healthAutoSync: healthAutoSync,
      permissionHandler: permissionHandler
    )
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
      const uploadHealthImage = (base64, mime) =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "health:autoUpload",
          base64: String(base64),
          mime: String(mime)
        });
      const analyzeHealthImage = (base64, mime, note) =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "health:autoAnalyze",
          base64: String(base64),
          mime: String(mime),
          note: String(note || "")
        });
      const healthAnalysisStatus = (jobId) =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "health:analysisStatus",
          jobId: String(jobId)
        });
      const acknowledgeHealthAnalysis = (jobId) =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "health:analysisAck",
          jobId: String(jobId)
        });
      const pendingHealthAnalyses = () =>
        window.webkit.messageHandlers.terrane.postMessage({
          kind: "health:pendingAnalyses"
        });
      Object.defineProperty(window, "terrane", {
        value: Object.freeze({
          invoke,
          uploadHealthImage,
          analyzeHealthImage,
          healthAnalysisStatus,
          acknowledgeHealthAnalysis,
          pendingHealthAnalyses
        }),
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
    weak var healthAutoSync: IOSHealthAutoSync?
    let permissionHandler: (TerraneApp, [String]) async -> Bool

    init(
      app: TerraneApp,
      runtime: any TerraneRuntime,
      healthAutoSync: IOSHealthAutoSync?,
      permissionHandler: @escaping (TerraneApp, [String]) async -> Bool
    ) {
      self.app = app
      self.runtime = runtime
      self.healthAutoSync = healthAutoSync
      self.permissionHandler = permissionHandler
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
        let kind = body["kind"] as? String
      else {
        replyHandler(nil, "terrane: rejected untrusted bridge message")
        return
      }
      if kind == "health:autoUpload" {
        guard app.id == "health",
          let healthAutoSync,
          let base64 = body["base64"] as? String,
          !base64.isEmpty,
          base64.count <= 16 * 1024 * 1024,
          let mime = body["mime"] as? String
        else {
          replyHandler(nil, "terrane: Health sync is unavailable")
          return
        }
        Task { @MainActor in
          do {
            let attachment = try await healthAutoSync.upload(base64: base64, mime: mime)
            replyHandler(
              [
                "ok": true,
                "attachmentId": attachment.id,
                "clientId": attachment.clientId,
              ],
              nil
            )
          } catch {
            replyHandler(nil, error.localizedDescription)
          }
        }
        return
      }
      if kind == "health:autoAnalyze" {
        guard app.id == "health",
          let healthAutoSync,
          let base64 = body["base64"] as? String,
          !base64.isEmpty,
          base64.count <= 16 * 1024 * 1024,
          let mime = body["mime"] as? String
        else {
          replyHandler(nil, "terrane: Health analysis sync is unavailable")
          return
        }
        let note = (body["note"] as? String).map { String($0.prefix(500)) } ?? ""
        Task { @MainActor in
          do {
            let submitted = try await healthAutoSync.submit(
              base64: base64,
              mime: mime,
              note: note
            )
            replyHandler(
              [
                "ok": true,
                "attachmentId": submitted.attachment.id,
                "jobId": submitted.job.id,
                "status": submitted.job.status,
              ],
              nil
            )
          } catch {
            replyHandler(nil, error.localizedDescription)
          }
        }
        return
      }
      if kind == "health:analysisStatus" {
        guard app.id == "health",
          let healthAutoSync,
          let jobID = body["jobId"] as? String,
          !jobID.isEmpty
        else {
          replyHandler(nil, "terrane: Health analysis status is unavailable")
          return
        }
        Task { @MainActor in
          do {
            let update = try await healthAutoSync.analysisUpdate(jobID: jobID)
            replyHandler(update.bridgeValue, nil)
          } catch {
            replyHandler(nil, error.localizedDescription)
          }
        }
        return
      }
      if kind == "health:analysisAck" {
        guard app.id == "health",
          let healthAutoSync,
          let jobID = body["jobId"] as? String,
          !jobID.isEmpty
        else {
          replyHandler(nil, "terrane: Health analysis acknowledgement is unavailable")
          return
        }
        Task { @MainActor in
          do {
            try await healthAutoSync.acknowledge(jobID: jobID)
            replyHandler(["ok": true], nil)
          } catch {
            replyHandler(nil, error.localizedDescription)
          }
        }
        return
      }
      if kind == "health:pendingAnalyses" {
        guard app.id == "health", let healthAutoSync else {
          replyHandler(nil, "terrane: pending Health analyses are unavailable")
          return
        }
        Task { @MainActor in
          let jobIDs = await healthAutoSync.pendingJobIDs()
          replyHandler(
            [
              "ok": true,
              "jobIds": jobIDs,
            ],
            nil
          )
        }
        return
      }
      guard kind == "invoke",
        let verb = body["verb"] as? String,
        !verb.isEmpty
      else {
        replyHandler(nil, "terrane: rejected unsupported bridge message")
        return
      }
      let arguments = (body["args"] as? [Any] ?? []).map {
        ($0 as? String) ?? String(describing: $0)
      }
      Task {
        do {
          let result = try await invokeWithPermissionRetry(
            verb: verb,
            arguments: arguments
          )
          replyHandler(result, nil)
        } catch {
          replyHandler(nil, error.localizedDescription)
        }
      }
    }

    private func invokeWithPermissionRetry(
      verb: String,
      arguments: [String]
    ) async throws -> String {
      do {
        return try await runtime.invoke(
          appID: app.id,
          verb: verb,
          arguments: arguments
        )
      } catch {
        let message = error.localizedDescription
        guard
          let resources = IOSPermissionRequestParser.parse(
            error: message,
            appID: app.id
          ),
          await permissionHandler(app, resources)
        else {
          throw error
        }
        for namespace in resources {
          _ = try await runtime.dispatch(
            command: "auth.grant",
            arguments: ["user:local-owner", app.id, namespace]
          )
        }
        return try await runtime.invoke(
          appID: app.id,
          verb: verb,
          arguments: arguments
        )
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

enum IOSPermissionRequestParser {
  static func parse(error: String, appID: String) -> [String]? {
    let prefix = "permission required for app \(appID): grant "
    guard error.hasPrefix(prefix) else { return nil }
    let tail = String(error.dropFirst(prefix.count))
    let resourcesPart = tail.split(separator: ";", maxSplits: 1).first.map(String.init) ?? tail
    let resources =
      resourcesPart
      .split(separator: ",")
      .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
    return resources.isEmpty ? nil : resources
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
