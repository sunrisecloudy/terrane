import Foundation
import WebKit

struct PermissionRequiredPrompt: Equatable {
  let appId: String
  let appName: String
  let missingResources: [String]
  let message: String

  static func parse(error: String, appId: String, appName: String) -> PermissionRequiredPrompt? {
    let prefix = "permission required for app \(appId): grant "
    guard error.hasPrefix(prefix) else { return nil }
    let tail = String(error.dropFirst(prefix.count))
    let resourcesPart = tail.split(separator: ";", maxSplits: 1).first.map(String.init) ?? tail
    let resources = resourcesPart
      .split(separator: ",")
      .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
      .filter { !$0.isEmpty }
    guard !resources.isEmpty else { return nil }
    return PermissionRequiredPrompt(
      appId: appId,
      appName: appName.isEmpty ? appId : appName,
      missingResources: resources,
      message: error
    )
  }
}

/// Bridges the app UI to terrane-core over the Terrane host C ABI.
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
  private let handle: OpaquePointer  // TerraneHandle*
  private let worker = DispatchQueue(label: "com.terrane.host.bridge", qos: .userInitiated)

  /// Called when a page renames its document via `terrane.setDocument(...)`.
  /// The host owns the top bar, so it updates the breadcrumb (and persists).
  var onDocumentSet: ((String) -> Void)?
  var onPermissionRequired: ((PermissionRequiredPrompt, @escaping (Bool) -> Void) -> Void)?

  var terraneHandle: OpaquePointer { handle }

  var selectedAppId: String { appId }

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
      replyInvokingSelectedApp(
        verb: verb, args: Self.stringArgs(from: body["args"]), replyHandler)
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
    case "builderGenerate":
      guard let request = body["request"] as? [String: Any] else {
        replyHandler(nil, "terrane: malformed builder generate message")
        return
      }
      worker.async { [weak self] in
        guard let self else {
          DispatchQueue.main.async {
            replyHandler(nil, "terrane: bridge is closed")
          }
          return
        }
        let result = self.generateDraft(request: request)
        DispatchQueue.main.async {
          self.replyObject(result, replyHandler)
        }
      }
    case "document:set":
      // Only the main app frame owns the breadcrumb; the shim is injected into
      // all frames, so ignore renames from nested frames (e.g. an App Builder
      // preview) that must not drive the host chrome.
      if message.frameInfo.isMainFrame {
        onDocumentSet?((body["name"] as? String) ?? "")
      }
      replyHandler("ok", nil)
    default:
      replyHandler(nil, "terrane: unknown bridge message")
    }
  }

  /// The JS a host calls (via `evaluateJavaScript`) to push the current
  /// document name / theme / locale down to the page, firing
  /// `terrane.onDocument` / `terrane.onTheme` / `terrane.onLocale`. Values are
  /// JSON-encoded so they cannot break out.
  static func applyStateJS(
    document: String?,
    theme: String?,
    locale: String? = nil,
    messages: [String: String]? = nil,
    dir: String? = nil
  ) -> String {
    var parts: [String] = []
    if let document {
      parts.append("document:\(jsonStringLiteral(document))")
    }
    if let theme {
      parts.append("theme:\(jsonStringLiteral(theme))")
    }
    if let locale {
      parts.append("locale:\(jsonStringLiteral(locale))")
    }
    if let dir {
      parts.append("dir:\(jsonStringLiteral(dir))")
    }
    if let messages {
      parts.append("messages:\(jsonObjectLiteral(messages))")
    }
    return "window.__terrane_apply && window.__terrane_apply({\(parts.joined(separator: ","))});"
  }

  /// A JSON object literal with every key and value routed through
  /// `jsonStringLiteral`, so a message-bundle string cannot break out of the
  /// evaluated JS — the same protection the document name has.
  static func jsonObjectLiteral(_ map: [String: String]) -> String {
    let body = map
      .sorted { $0.key < $1.key }
      .map { "\(jsonStringLiteral($0.key)):\(jsonStringLiteral($0.value))" }
      .joined(separator: ",")
    return "{\(body)}"
  }

  /// The writing direction for a locale code — parity with terrane-i18n's
  /// `dir_for` (only Arabic is RTL in the initial set).
  static func dir(for code: String) -> String {
    code.lowercased() == "ar" ? "rtl" : "ltr"
  }

  /// Best supported locale for the system's preferred languages, via the core
  /// negotiation (one source of truth with the web host's Accept-Language).
  /// Handle-free. Falls back to "en".
  static func negotiateLocale(_ prefs: [String]) -> String {
    let header = prefs.joined(separator: ",")
    var out: UnsafeMutablePointer<CChar>?
    var err: UnsafeMutablePointer<CChar>?
    let rc = header.withCString { terrane_i18n_negotiate($0, &out, &err) }
    defer {
      if let out { terrane_string_free(out) }
      if let err { terrane_string_free(err) }
    }
    if rc == 0, let out {
      return String(cString: out)
    }
    return "en"
  }

  /// Seed the public i18n bucket from checked-in catalogs under `path`
  /// (idempotent). Best-effort: errors are logged, not fatal, so a missing
  /// catalog just leaves apps on the English fallback.
  func i18nImport(path: String) {
    var out: UnsafeMutablePointer<CChar>?
    var err: UnsafeMutablePointer<CChar>?
    let rc = path.withCString { terrane_i18n_import(handle, $0, &out, &err) }
    defer {
      if let out { terrane_string_free(out) }
      if let err { terrane_string_free(err) }
    }
    if rc != 0, let err {
      NSLog("terrane-host: i18n seed skipped: \(String(cString: err))")
    }
  }

  /// The localized message bundle for `code` as `[key: value]`. `appId` empty =
  /// the shell-chrome ("system") bundle; otherwise the app frame bundle
  /// (system + that app's domain). English is the fallback layer.
  func i18nBundle(code: String, appId: String) -> [String: String] {
    var out: UnsafeMutablePointer<CChar>?
    var err: UnsafeMutablePointer<CChar>?
    let rc = code.withCString { codeC in
      appId.withCString { appC in
        terrane_i18n_bundle(handle, codeC, appC, &out, &err)
      }
    }
    defer {
      if let out { terrane_string_free(out) }
      if let err { terrane_string_free(err) }
    }
    guard rc == 0, let out,
      let data = String(cString: out).data(using: .utf8),
      let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else {
      return [:]
    }
    var map: [String: String] = [:]
    for (key, value) in obj {
      if let string = value as? String {
        map[key] = string
      }
    }
    return map
  }

  /// Strip control/format characters, collapse whitespace, cap length — parity
  /// with the web shell's setDocName so a page-supplied name cannot spoof the
  /// trusted breadcrumb chrome.
  static func sanitizeDocName(_ raw: String) -> String {
    let stripped = raw.unicodeScalars.filter { scalar in
      !(scalar.value < 0x20 || (scalar.value >= 0x7f && scalar.value <= 0x9f)
        || (scalar.value >= 0x200b && scalar.value <= 0x200f)
        || (scalar.value >= 0x2028 && scalar.value <= 0x202e)
        || (scalar.value >= 0x2066 && scalar.value <= 0x2069)
        || scalar.value == 0xfeff)
    }
    let collapsed = String(String.UnicodeScalarView(stripped))
      .components(separatedBy: .whitespacesAndNewlines)
      .filter { !$0.isEmpty }
      .joined(separator: " ")
    let capped = String(collapsed.prefix(120))
    return capped.isEmpty ? "Untitled" : capped
  }

  /// Minimal JSON string literal (also escapes `<` and line separators) so a
  /// document name can be embedded in evaluated JS without breaking it.
  static func jsonStringLiteral(_ value: String) -> String {
    var out = "\""
    for scalar in value.unicodeScalars {
      switch scalar {
      case "\"": out += "\\\""
      case "\\": out += "\\\\"
      case "<": out += "\\u003c"
      case "\n": out += "\\n"
      case "\r": out += "\\r"
      case "\u{2028}": out += "\\u2028"
      case "\u{2029}": out += "\\u2029"
      default:
        if scalar.value < 0x20 {
          out += String(format: "\\u%04x", scalar.value)
        } else {
          out.unicodeScalars.append(scalar)
        }
      }
    }
    out += "\""
    return out
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

  func blobAsset(appId: String, name: String) -> AppAssetResult {
    let (ok, payload) = appId.withCString { appC in
      name.withCString { nameC in
        callBlobRead(appId: appC, name: nameC)
      }
    }
    guard ok else {
      return .failure(payload)
    }
    guard let object = Self.jsonObject(from: payload),
      let content = object["content"] as? String,
      let data = Data(base64Encoded: content)
    else {
      return .failure("terrane_blob_read returned malformed JSON")
    }
    let contentType = (object["contentType"] as? String) ?? "application/octet-stream"
    return .success(AppAsset(data: data, contentType: contentType))
  }

  func invokeSelectedApp(verb: String, args: [String]) -> (Bool, String) {
    guard !appId.isEmpty else {
      return (false, "terrane: no app selected")
    }
    return hostRun(argv: [verb] + args)
  }

  func openExternal(target: String) -> (Bool, String) {
    target.withCString { targetC -> (Bool, String) in
      var out: UnsafeMutablePointer<CChar>?
      var err: UnsafeMutablePointer<CChar>?
      let rc = terrane_open_target(handle, targetC, &out, &err)
      return output(rc: rc, out: out, err: err, label: "terrane_open_target")
    }
  }

  func grant(app: String, namespace: String) -> (Bool, String) {
    dispatch(command: "auth.grant", argv: ["user:local-owner", app, namespace])
  }

  private func replyInvokingSelectedApp(
    verb: String,
    args: [String],
    _ replyHandler: @escaping (Any?, String?) -> Void
  ) {
    let result = invokeSelectedApp(verb: verb, args: args)
    guard !result.0,
      let prompt = PermissionRequiredPrompt.parse(
        error: result.1, appId: appId, appName: appName),
      let onPermissionRequired
    else {
      replyString(result, replyHandler)
      return
    }

    onPermissionRequired(prompt) { [weak self] approved in
      guard let self else {
        replyHandler(nil, result.1)
        return
      }
      guard approved else {
        replyHandler(nil, result.1)
        return
      }
      self.replyString(self.invokeSelectedApp(verb: verb, args: args), replyHandler)
    }
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

  private func generateDraft(request: [String: Any]) -> (Bool, Any) {
    let id = (request["id"] as? String) ?? ""
    let name = (request["name"] as? String) ?? ""
    let prompt = (request["prompt"] as? String) ?? ""
    let harness =
      (request["harness"] as? String)
      ?? (request["agent"] as? String)
      ?? "codex"

    let (ok, payload) = id.withCString { idC in
      name.withCString { nameC in
        prompt.withCString { promptC in
          harness.withCString { harnessC in
            callBuilderGenerate(appId: idC, name: nameC, prompt: promptC, harness: harnessC)
          }
        }
      }
    }
    guard ok else {
      return (false, payload)
    }
    guard let object = Self.jsonObject(from: payload),
      object["id"] is String,
      object["status"] is String,
      object["files"] is [Any]
    else {
      return (false, "terrane_builder_generate returned malformed JSON")
    }
    return (true, object)
  }

  private func previewInvoke(previewId: String, verb: String, args: [String]) -> (Bool, String) {
    return previewId.withCString { previewC in
      verb.withCString { verbC in
        var cargs: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) }
        defer {
          for carg in cargs {
            free(carg)
          }
        }

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
      defer {
        for carg in cargs {
          free(carg)
        }
      }

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
      defer {
        for carg in cargs {
          free(carg)
        }
      }

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

  private func callBlobRead(
    appId: UnsafePointer<CChar>,
    name: UnsafePointer<CChar>
  ) -> (Bool, String) {
    var out: UnsafeMutablePointer<CChar>?
    var err: UnsafeMutablePointer<CChar>?
    let rc = terrane_blob_read(handle, appId, name, &out, &err)
    return output(rc: rc, out: out, err: err, label: "terrane_blob_read")
  }

  private func callBuilderGenerate(
    appId: UnsafePointer<CChar>,
    name: UnsafePointer<CChar>,
    prompt: UnsafePointer<CChar>,
    harness: UnsafePointer<CChar>
  ) -> (Bool, String) {
    var out: UnsafeMutablePointer<CChar>?
    var err: UnsafeMutablePointer<CChar>?
    let rc = terrane_builder_generate(handle, appId, name, prompt, harness, &out, &err)
    return output(rc: rc, out: out, err: err, label: "terrane_builder_generate")
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

      // Top-bar document/theme/locale state, kept in sync with the native host.
      var docState = "";
      var themeState = "system";
      var localeState = "en";
      var messagesState = {};
      var dirState = "ltr";
      var docSubs = [];
      var themeSubs = [];
      var localeSubs = [];
      var messagesSubs = [];
      function notify(subs, value) {
        for (var i = 0; i < subs.length; i++) {
          try { subs[i](value); } catch (_) {}
        }
      }
      function copyMessages() {
        var copy = {};
        for (var k in messagesState) {
          if (Object.prototype.hasOwnProperty.call(messagesState, k)) copy[k] = messagesState[k];
        }
        return copy;
      }
      function translate(key, params) {
        key = String(key == null ? "" : key);
        var template = Object.prototype.hasOwnProperty.call(messagesState, key)
          ? messagesState[key]
          : (params && Object.prototype.hasOwnProperty.call(params, "default")
              ? String(params.default) : key);
        if (!params) return template;
        return String(template).replace(/\\{(\\w+)\\}/g, function (m, name) {
          if (name === "default") return m;
          return Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : m;
        });
      }
      function unsubscriber(list, cb) {
        return function () {
          for (var i = list.length - 1; i >= 0; i--) {
            if (list[i] === cb) list.splice(i, 1);
          }
        };
      }
      // The native host calls this (via evaluateJavaScript) to push the
      // current document name / theme down; it is intentionally not on the
      // frozen `terrane` object.
      window.__terrane_apply = function (state) {
        if (!state) return;
        if (typeof state.document === "string") {
          docState = state.document;
          notify(docSubs, docState);
        }
        if (typeof state.theme === "string") {
          themeState = state.theme;
          notify(themeSubs, themeState);
        }
        if (typeof state.locale === "string") {
          localeState = state.locale || "en";
          notify(localeSubs, localeState);
        }
        if (state.messages && typeof state.messages === "object") {
          messagesState = state.messages;
          notify(messagesSubs, copyMessages());
        }
        if (typeof state.dir === "string") {
          dirState = state.dir === "rtl" ? "rtl" : "ltr";
        }
      };

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
        blobUrl: function (name) {
          var app = window.location.host || "";
          return "terrane-app://" + app + "/blob/" + encodeURIComponent(String(name == null ? "" : name));
        },
        preview: function (files) {
          return post({ kind: "preview", files: files });
        },
        builderGenerate: function (request) {
          request = request || {};
          return post({
            kind: "builderGenerate",
            request: {
              id: String(request.id || ""),
              name: String(request.name || ""),
              prompt: String(request.prompt || ""),
              harness: String(request.harness || request.agent || "codex")
            }
          });
        },
        // --- Top-bar document/theme (host chrome) — parity with the web host ---
        getDocument: function () {
          return docState;
        },
        setDocument: function (name) {
          var clean = String(name == null ? "" : name);
          docState = clean;
          post({ kind: "document:set", name: clean });
        },
        onDocument: function (cb) {
          if (typeof cb !== "function") return function () {};
          docSubs.push(cb);
          if (docState) { try { cb(docState); } catch (_) {} }
          return unsubscriber(docSubs, cb);
        },
        getTheme: function () {
          return themeState;
        },
        onTheme: function (cb) {
          if (typeof cb !== "function") return function () {};
          themeSubs.push(cb);
          try { cb(themeState); } catch (_) {}
          return unsubscriber(themeSubs, cb);
        },
        // --- Localization (host chrome) — parity with the web host ---
        getLocale: function () {
          return localeState;
        },
        onLocale: function (cb) {
          if (typeof cb !== "function") return function () {};
          localeSubs.push(cb);
          try { cb(localeState); } catch (_) {}
          return unsubscriber(localeSubs, cb);
        },
        getMessages: function () {
          return copyMessages();
        },
        onMessages: function (cb) {
          if (typeof cb !== "function") return function () {};
          messagesSubs.push(cb);
          try { cb(copyMessages()); } catch (_) {}
          return unsubscriber(messagesSubs, cb);
        },
        getDir: function () {
          return dirState;
        },
        t: function (key, params) {
          return translate(key, params);
        }
      });
      Object.defineProperty(window, "terrane", {
        configurable: true,
        value: api
      });
    })();
    """
}
