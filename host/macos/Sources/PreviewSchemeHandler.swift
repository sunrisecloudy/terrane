import Foundation
import WebKit

struct PreviewAsset {
  let content: String
  let contentType: String

  var data: Data {
    Data(content.utf8)
  }

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

enum PreviewAssetResult {
  case success(PreviewAsset)
  case failure(String)
}

final class PreviewSchemeHandler: NSObject, WKURLSchemeHandler {
  static let scheme = "terrane-preview"

  private weak var bridge: TerraneBridge?

  init(bridge: TerraneBridge) {
    self.bridge = bridge
    super.init()
  }

  func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
    guard let url = urlSchemeTask.request.url,
      let request = PreviewRequest(url: url)
    else {
      respond(to: urlSchemeTask, status: 404, message: "preview asset not found")
      return
    }

    guard let bridge else {
      respond(to: urlSchemeTask, status: 500, message: "terrane bridge is closed")
      return
    }

    switch bridge.previewAsset(previewId: request.previewId, relPath: request.relPath) {
    case .success(let asset):
      respond(to: urlSchemeTask, asset: asset)
    case .failure(let message):
      respond(
        to: urlSchemeTask,
        status: message.localizedCaseInsensitiveContains("not found") ? 404 : 500,
        message: message
      )
    }
  }

  func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
    // FFI preview asset reads are synchronous for now; there is no in-flight work to cancel.
  }

  private func respond(to task: WKURLSchemeTask, asset: PreviewAsset) {
    let data = asset.data
    let response = response(
      for: task,
      status: 200,
      contentType: asset.contentType,
      expectedLength: data.count,
      mimeType: asset.mimeType,
      encoding: asset.textEncodingName
    )
    task.didReceive(response)
    task.didReceive(data)
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

private struct PreviewRequest {
  let previewId: String
  let relPath: String

  init?(url: URL) {
    guard url.scheme == PreviewSchemeHandler.scheme,
      let host = url.host?.removingPercentEncoding,
      !host.isEmpty
    else {
      return nil
    }

    let path = url.path
    let resolvedRelPath: String
    if path == "/frame" || path == "/frame/" {
      resolvedRelPath = ""
    } else if path.hasPrefix("/frame/") {
      let rawRelPath = String(path.dropFirst("/frame/".count))
      resolvedRelPath = rawRelPath.removingPercentEncoding ?? rawRelPath
    } else {
      return nil
    }

    guard !resolvedRelPath.split(separator: "/").contains("..") else {
      return nil
    }
    previewId = host
    relPath = resolvedRelPath
  }
}
