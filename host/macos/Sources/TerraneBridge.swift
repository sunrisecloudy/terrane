import Foundation
import WebKit

/// Bridges the app UI to terrane-core over the terrane-ffi C ABI.
///
/// The selected app path calls `terrane.invoke(verb, ...args)` and the App
/// Builder path calls `terrane.preview(files)`; preview frames call the same
/// `terrane.invoke`, routed by preview id. All paths settle the JS Promise with
/// backend output or an error string.
final class TerraneBridge: NSObject, WKScriptMessageHandlerWithReply {
    private var appId = ""
    private var appName = ""
    private var appSource = ""
    private var catalogedAppIds = Set<String>()
    private let handle: OpaquePointer // TerraneHandle*

    init?(home: URL) {
        guard let handle = home.path.withCString({ terrane_open($0) }) else {
            return nil
        }
        self.handle = handle
        super.init()
    }

    /// Install the JS shim (at document start) + the reply message handler.
    func install(into ucc: WKUserContentController) {
        ucc.addUserScript(
            WKUserScript(source: Self.shim, injectionTime: .atDocumentStart, forMainFrameOnly: false)
        )
        ucc.addScriptMessageHandler(self, contentWorld: .page, name: "terrane")
    }

    func select(app: TerraneApp) {
        appId = app.id
        appName = app.name
        appSource = app.directory.path
        ensureSelectedAppCataloged()
    }

    func clearSelection() {
        appId = ""
        appName = ""
        appSource = ""
    }

    func close() {
        terrane_close(handle)
    }

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any] else {
            replyHandler(nil, "terrane: malformed message")
            return
        }

        switch (body["kind"] as? String) ?? "invoke" {
        case "invoke":
            guard let verb = body["verb"] as? String else {
                replyHandler(nil, "terrane: malformed invoke message")
                return
            }
            replyString(callSelectedApp(verb: verb, args: Self.stringArgs(from: body["args"])), replyHandler)
        case "preview":
            guard let files = body["files"] else {
                replyHandler(nil, "terrane: malformed preview message")
                return
            }
            replyObject(createPreview(files: files), replyHandler)
        case "previewInvoke":
            guard let previewId = body["previewId"] as? String,
                  let verb = body["verb"] as? String
            else {
                replyHandler(nil, "terrane: malformed preview invoke message")
                return
            }
            replyString(
                previewInvoke(previewId: previewId, verb: verb, args: Self.stringArgs(from: body["args"])),
                replyHandler
            )
        default:
            replyHandler(nil, "terrane: unknown bridge message")
        }
    }

    func previewAsset(previewId: String, relPath: String) -> PreviewAssetResult {
        let (ok, payload) = previewId.withCString { previewC in
            relPath.withCString { relPathC in
                callPreviewAsset(previewId: previewC, relPath: relPathC)
            }
        }
        guard ok else {
            return .failure(payload)
        }
        guard let object = Self.jsonObject(from: payload),
              let content = object["content"] as? String
        else {
            return .failure("terrane_preview_asset returned malformed JSON")
        }
        let contentType = (object["contentType"] as? String) ?? "application/octet-stream"
        return .success(PreviewAsset(content: content, contentType: contentType))
    }

    private func callSelectedApp(verb: String, args: [String]) -> (Bool, String) {
        guard !appId.isEmpty else {
            return (false, "terrane: no app selected")
        }
        return hostRun(argv: [verb] + args)
    }

    private func createPreview(files: Any) -> (Bool, Any) {
        guard JSONSerialization.isValidJSONObject(files),
              let data = try? JSONSerialization.data(withJSONObject: files),
              let json = String(data: data, encoding: .utf8)
        else {
            return (false, "terrane: preview files must be JSON")
        }

        let (ok, payload) = json.withCString { filesC in
            callPreviewCreate(filesJSON: filesC)
        }
        guard ok else {
            return (false, payload)
        }
        guard let object = Self.jsonObject(from: payload),
              object["id"] is String,
              object["frameUrl"] is String
        else {
            return (false, "terrane_preview_create returned malformed JSON")
        }
        return (true, object)
    }

    private func previewInvoke(previewId: String, verb: String, args: [String]) -> (Bool, String) {
        return previewId.withCString { previewC in
            verb.withCString { verbC in
                var cargs: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) }
                defer { cargs.forEach { free($0) } }

                var out: UnsafeMutablePointer<CChar>?
                var err: UnsafeMutablePointer<CChar>?
                let rc: Int32
                if cargs.isEmpty {
                    rc = terrane_preview_invoke(handle, previewC, verbC, 0, nil, &out, &err)
                } else {
                    rc = cargs.withUnsafeMutableBufferPointer { buf -> Int32 in
                        buf.baseAddress!.withMemoryRebound(
                            to: UnsafePointer<CChar>?.self, capacity: buf.count
                        ) { argvPtr in
                            terrane_preview_invoke(handle, previewC, verbC, args.count, argvPtr, &out, &err)
                        }
                    }
                }
                return output(rc: rc, out: out, err: err, label: "terrane_preview_invoke")
            }
        }
    }

    private func replyString(
        _ result: (Bool, String),
        _ replyHandler: @escaping (Any?, String?) -> Void
    ) {
        let (ok, payload) = result
        if ok {
            replyHandler(payload, nil)
        } else {
            replyHandler(nil, payload)
        }
    }

    private func replyObject(
        _ result: (Bool, Any),
        _ replyHandler: @escaping (Any?, String?) -> Void
    ) {
        let (ok, payload) = result
        if ok {
            replyHandler(payload, nil)
        } else {
            replyHandler(nil, String(describing: payload))
        }
    }

    private func ensureSelectedAppCataloged() {
        guard !appId.isEmpty, !catalogedAppIds.contains(appId) else { return }

        let (ok, payload) = dispatch(
            command: "app.add",
            argv: [appId, appName, "--source", appSource]
        )
        if ok || payload == "app already exists: \(appId)" {
            catalogedAppIds.insert(appId)
        } else {
            NSLog("terrane-host: cannot catalog \(appId): \(payload)")
        }
    }

    /// Marshal Swift strings → C argv, call terrane_host_run, return (ok, text).
    private func hostRun(argv: [String]) -> (Bool, String) {
        appId.withCString { appC -> (Bool, String) in
            var cargs: [UnsafeMutablePointer<CChar>?] = argv.map { strdup($0) }
            defer { cargs.forEach { free($0) } }

            var out: UnsafeMutablePointer<CChar>?
            var err: UnsafeMutablePointer<CChar>?
            let rc = cargs.withUnsafeMutableBufferPointer { buf -> Int32 in
                buf.baseAddress!.withMemoryRebound(
                    to: UnsafePointer<CChar>?.self, capacity: buf.count
                ) { argvPtr in
                    terrane_host_run(handle, appC, argv.count, argvPtr, &out, &err)
                }
            }

            return output(rc: rc, out: out, err: err, label: "terrane_host_run")
        }
    }

    /// Marshal Swift strings → C argv, call terrane_dispatch, return (ok, text).
    private func dispatch(command: String, argv: [String]) -> (Bool, String) {
        command.withCString { commandC -> (Bool, String) in
            var cargs: [UnsafeMutablePointer<CChar>?] = argv.map { strdup($0) }
            defer { cargs.forEach { free($0) } }

            var out: UnsafeMutablePointer<CChar>?
            var err: UnsafeMutablePointer<CChar>?
            let rc = cargs.withUnsafeMutableBufferPointer { buf -> Int32 in
                buf.baseAddress!.withMemoryRebound(
                    to: UnsafePointer<CChar>?.self, capacity: buf.count
                ) { argvPtr in
                    terrane_dispatch(handle, commandC, argv.count, argvPtr, &out, &err)
                }
            }

            return output(rc: rc, out: out, err: err, label: "terrane_dispatch")
        }
    }

    private func callPreviewCreate(
        filesJSON: UnsafePointer<CChar>
    ) -> (Bool, String) {
        var out: UnsafeMutablePointer<CChar>?
        var err: UnsafeMutablePointer<CChar>?
        let rc = terrane_preview_create(handle, filesJSON, &out, &err)
        return output(rc: rc, out: out, err: err, label: "terrane_preview_create")
    }

    private func callPreviewAsset(
        previewId: UnsafePointer<CChar>,
        relPath: UnsafePointer<CChar>
    ) -> (Bool, String) {
        var out: UnsafeMutablePointer<CChar>?
        var err: UnsafeMutablePointer<CChar>?
        let rc = terrane_preview_read_asset(handle, previewId, relPath, &out, &err)
        return output(rc: rc, out: out, err: err, label: "terrane_preview_read_asset")
    }

    private func output(
        rc: Int32,
        out: UnsafeMutablePointer<CChar>?,
        err: UnsafeMutablePointer<CChar>?,
        label: String
    ) -> (Bool, String) {
        if rc == 0, let o = out {
            defer { terrane_string_free(o) }
            return (true, String(cString: o))
        }
        if let e = err {
            defer { terrane_string_free(e) }
            return (false, String(cString: e))
        }
        return (false, "\(label) failed (rc=\(rc))")
    }

    private static func stringArgs(from value: Any?) -> [String] {
        if let args = value as? [String] {
            return args
        }
        if let args = value as? [Any] {
            return args.map { String(describing: $0) }
        }
        return []
    }

    private static func jsonObject(from text: String) -> [String: Any]? {
        guard let data = text.data(using: .utf8),
              let parsed = try? JSONSerialization.jsonObject(with: data)
        else {
            return nil
        }
        return parsed as? [String: Any]
    }

    /// Injected at document start. `invoke` returns the Promise that
    /// WKScriptMessageHandlerWithReply settles with the backend output / error.
    private static let shim = """
    (function () {
      function post(message) {
        return window.webkit.messageHandlers.terrane.postMessage(message);
      }
      function currentPreviewId() {
        if (window.location.protocol !== "terrane-preview:") return "";
        var raw = window.location.host || "";
        try {
          return decodeURIComponent(raw);
        } catch (_) {
          return raw;
        }
      }
      var api = Object.freeze({
        invoke: function (verb) {
          var args = Array.prototype.slice.call(arguments, 1).map(String);
          var previewId = currentPreviewId();
          if (previewId) {
            return post({
              kind: "previewInvoke",
              previewId: previewId,
              verb: String(verb),
              args: args
            });
          }
          return post({ kind: "invoke", verb: String(verb), args: args });
        },
        preview: function (files) {
          return post({ kind: "preview", files: files });
        }
      });
      Object.defineProperty(window, "terrane", {
        configurable: true,
        value: api
      });
    })();
    """
}
