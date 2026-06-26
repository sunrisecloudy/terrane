import Foundation
import WebKit

/// Bridges the app UI to terrane-core over the terrane-ffi C ABI.
///
/// The page calls `terrane.invoke(verb, ...args)` → posts `{verb, args}` to the
/// `terrane` message handler → we call `terrane_host_run(handle, app, [verb, …])`
/// → the backend's output string settles the JS Promise. The app id is fixed at
/// construction, so the page can only ever act as its own app (sandbox).
final class TerraneBridge: NSObject, WKScriptMessageHandlerWithReply {
    private let appId: String
    private let handle: OpaquePointer // TerraneHandle*

    init?(home: URL, appId: String) {
        guard let handle = home.path.withCString({ terrane_open($0) }) else {
            return nil
        }
        self.appId = appId
        self.handle = handle
        super.init()
    }

    /// Install the JS shim (at document start) + the reply message handler.
    func install(into ucc: WKUserContentController) {
        ucc.addUserScript(
            WKUserScript(source: Self.shim, injectionTime: .atDocumentStart, forMainFrameOnly: true)
        )
        ucc.addScriptMessageHandler(self, contentWorld: .page, name: "terrane")
    }

    func close() {
        terrane_close(handle)
    }

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any],
              let verb = body["verb"] as? String
        else {
            replyHandler(nil, "terrane: malformed invoke message")
            return
        }
        let extra = (body["args"] as? [String]) ?? []
        let (ok, payload) = hostRun(argv: [verb] + extra)
        if ok {
            replyHandler(payload, nil)
        } else {
            replyHandler(nil, payload)
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

            if rc == 0, let o = out {
                defer { terrane_string_free(o) }
                return (true, String(cString: o))
            }
            if let e = err {
                defer { terrane_string_free(e) }
                return (false, String(cString: e))
            }
            return (false, "terrane_host_run failed (rc=\(rc))")
        }
    }

    /// Injected at document start. `invoke` returns the Promise that
    /// WKScriptMessageHandlerWithReply settles with the backend output / error.
    private static let shim = """
    Object.defineProperty(window, "terrane", { value: Object.freeze({
      invoke: function (verb) {
        var args = Array.prototype.slice.call(arguments, 1).map(String);
        return window.webkit.messageHandlers.terrane.postMessage({ verb: String(verb), args: args });
      }
    })});
    """
}
