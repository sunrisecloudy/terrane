import Foundation
import WebKit

struct AppAsset {
  let data: Data
  let contentType: String

  var mimeType: String {
    contentType
      .split(separator: ";", maxSplits: 1)
      .first
      .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
      .flatMap { $0.isEmpty ? nil : $0 }
      ?? "application/octet-stream"
  }

  var textEncodingName: String? {
    for part in contentType.split(separator: ";").dropFirst() {
      let pieces = part.split(separator: "=", maxSplits: 1)
      guard pieces.count == 2,
        pieces[0].trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "charset"
      else {
        continue
      }
      let charset = pieces[1].trimmingCharacters(in: .whitespacesAndNewlines)
      return charset.isEmpty ? nil : String(charset)
    }
    return nil
  }
}

enum AppAssetResult {
  case success(AppAsset)
  case failure(status: Int, message: String)
}

enum AppAssetBase {
  case uiDirectory
  case appRoot
  case blob
}

enum AppAssetStore {
  static func asset(
    apps: [TerraneApp],
    appId: String,
    relPath: String,
    base: AppAssetBase = .uiDirectory
  ) -> AppAssetResult {
    guard let app = apps.first(where: { $0.id == appId }) else {
      return .failure(status: 404, message: "app not found: \(appId)")
    }
    guard !relPath.split(separator: "/").contains("..") else {
      return .failure(status: 403, message: "app asset path escapes app root")
    }

    let baseURL =
      base == .appRoot
      ? app.directory.standardizedFileURL
      : app.uiURL.deletingLastPathComponent().standardizedFileURL
    let target =
      relPath.isEmpty
      ? app.uiURL.standardizedFileURL
      : baseURL.appendingPathComponent(relPath).standardizedFileURL
    let appRoot = app.directory.standardizedFileURL.path
    guard target.path == appRoot || target.path.hasPrefix(appRoot + "/") else {
      return .failure(status: 403, message: "app asset path escapes app root")
    }

    do {
      let values = try target.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey])
      guard values.isRegularFile == true, values.isDirectory != true else {
        return .failure(status: 404, message: "app asset not found")
      }
      return .success(
        AppAsset(
          data: try Data(contentsOf: target),
          contentType: contentType(for: target.pathExtension)
        )
      )
    } catch {
      return .failure(status: 404, message: "app asset not found: \(relPath)")
    }
  }

  private static func contentType(for ext: String) -> String {
    switch ext.lowercased() {
    case "css":
      return "text/css; charset=utf-8"
    case "htm", "html":
      return "text/html; charset=utf-8"
    case "js", "mjs":
      return "text/javascript; charset=utf-8"
    case "json":
      return "application/json; charset=utf-8"
    case "svg":
      return "image/svg+xml; charset=utf-8"
    case "txt":
      return "text/plain; charset=utf-8"
    default:
      return "application/octet-stream"
    }
  }
}

final class AppSchemeHandler: NSObject, WKURLSchemeHandler {
  static let scheme = "terrane-app"

  private let apps: () -> [TerraneApp]
  private weak var bridge: TerraneBridge?

  init(bridge: TerraneBridge, apps: @escaping () -> [TerraneApp]) {
    self.bridge = bridge
    self.apps = apps
    super.init()
  }

  static func frameURL(for app: TerraneApp) -> URL {
    var components = URLComponents()
    components.scheme = scheme
    components.host = app.id
    components.path = "/frame/"
    return components.url!
  }

  static func assetURL(for app: TerraneApp, relPath: String) -> URL? {
    guard !relPath.isEmpty,
      !relPath.hasPrefix("/"),
      !relPath.contains("\\"),
      !relPath.split(separator: "/").contains("..")
    else {
      return nil
    }
    var components = URLComponents()
    components.scheme = scheme
    components.host = app.id
    components.path = "/asset/" + relPath
    return components.url
  }

  func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
    guard let url = urlSchemeTask.request.url,
      let request = AppAssetRequest(url: url)
    else {
      respond(to: urlSchemeTask, status: 404, message: "app asset not found")
      return
    }

    if request.base == .blob {
      switch bridge?.blobAsset(appId: request.appId, name: request.relPath) {
      case .success(let asset):
        respond(to: urlSchemeTask, asset: asset)
      case .failure(let message):
        respond(to: urlSchemeTask, status: 404, message: message)
      case .none:
        respond(to: urlSchemeTask, status: 500, message: "terrane bridge is closed")
      }
    } else {
      switch AppAssetStore.asset(
        apps: apps(), appId: request.appId, relPath: request.relPath, base: request.base
      ) {
      case .success(let asset):
        respond(to: urlSchemeTask, asset: asset)
      case .failure(let status, let message):
        respond(to: urlSchemeTask, status: status, message: message)
      }
    }
  }

  func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
    // Disk reads are synchronous for now; there is no in-flight work to cancel.
  }

  private func respond(to task: WKURLSchemeTask, asset: AppAsset) {
    let response = response(
      for: task,
      status: 200,
      contentType: asset.contentType,
      expectedLength: asset.data.count,
      mimeType: asset.mimeType,
      encoding: asset.textEncodingName
    )
    task.didReceive(response)
    task.didReceive(asset.data)
    task.didFinish()
  }

  private func respond(to task: WKURLSchemeTask, status: Int, message: String) {
    let body = Data(message.utf8)
    let response = response(
      for: task,
      status: status,
      contentType: "text/plain; charset=utf-8",
      expectedLength: body.count,
      mimeType: "text/plain",
      encoding: "utf-8"
    )
    task.didReceive(response)
    task.didReceive(body)
    task.didFinish()
  }

  private func response(
    for task: WKURLSchemeTask,
    status: Int,
    contentType: String,
    expectedLength: Int,
    mimeType: String,
    encoding: String?
  ) -> URLResponse {
    let url = task.request.url ?? URL(string: "\(Self.scheme)://invalid/")!
    let headers = [
      "Content-Type": contentType,
      "Content-Length": String(expectedLength),
      "Cache-Control": "no-store, max-age=0",
      "Pragma": "no-cache",
    ]
    if let response = HTTPURLResponse(
      url: url,
      statusCode: status,
      httpVersion: "HTTP/1.1",
      headerFields: headers
    ) {
      return response
    }
    return URLResponse(
      url: url,
      mimeType: mimeType,
      expectedContentLength: expectedLength,
      textEncodingName: encoding
    )
  }
}

private struct AppAssetRequest {
  let appId: String
  let relPath: String
  let base: AppAssetBase

  init?(url: URL) {
    guard url.scheme == AppSchemeHandler.scheme,
      let host = url.host?.removingPercentEncoding,
      !host.isEmpty
    else {
      return nil
    }

    let path = url.path
    let resolvedRelPath: String
    if path == "/frame" || path == "/frame/" {
      resolvedRelPath = ""
      base = .uiDirectory
    } else if path.hasPrefix("/frame/") {
      let rawRelPath = String(path.dropFirst("/frame/".count))
      resolvedRelPath = rawRelPath.removingPercentEncoding ?? rawRelPath
      base = .uiDirectory
    } else if path.hasPrefix("/asset/") {
      let rawRelPath = String(path.dropFirst("/asset/".count))
      guard !rawRelPath.isEmpty else {
        return nil
      }
      resolvedRelPath = rawRelPath.removingPercentEncoding ?? rawRelPath
      base = .appRoot
    } else if path.hasPrefix("/blob/") {
      let rawRelPath = String(path.dropFirst("/blob/".count))
      guard !rawRelPath.isEmpty else {
        return nil
      }
      resolvedRelPath = rawRelPath.removingPercentEncoding ?? rawRelPath
      base = .blob
    } else {
      return nil
    }

    guard !resolvedRelPath.split(separator: "/").contains("..") else {
      return nil
    }
    appId = host
    relPath = resolvedRelPath
  }
}
