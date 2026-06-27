import AppKit
import Foundation

struct SourceFile: Equatable {
  let relativePath: String
  let url: URL
}

struct TerraneBuildResult: Equatable {
  let dist: URL
  let files: Int
}

enum SourceEditorError: LocalizedError {
  case fileTooLarge(String)
  case invalidPath(String)
  case invalidText(String)
  case malformedBuildOutput(String)
  case writeFailed(String)
  case buildFailed(String)

  var errorDescription: String? {
    switch self {
    case .fileTooLarge(let path):
      return "\(path) is too large to edit in Terrane."
    case .invalidPath(let path):
      return "\(path) is not an editable app source file."
    case .invalidText(let path):
      return "\(path) is not valid UTF-8 text."
    case .malformedBuildOutput(let output):
      return "terrane_build_app returned malformed output: \(output)"
    case .writeFailed(let message):
      return message
    case .buildFailed(let message):
      return message
    }
  }
}

enum SourceEditorModel {
  private static let editableExtensions: Set<String> = [
    "css", "htm", "html", "js", "json", "jsx", "md", "mjs", "svg", "ts", "tsx", "txt",
  ]
  private static let skippedDirectories: Set<String> = [
    ".derived", ".git", ".terrane", "build", "node_modules", "target", "vendor",
  ]
  private static let maxEditableBytes = 1_000_000

  static func requiresBuild(app: TerraneApp) -> Bool {
    guard
      let manifest = manifestObject(in: app.directory),
      let frontend = manifest["frontend"] as? [String: Any],
      let tool = frontend["tool"] as? String
    else {
      return false
    }
    return tool.trimmingCharacters(in: .whitespacesAndNewlines) == "terrane-app-build"
  }

  static func editableFiles(for app: TerraneApp) throws -> [SourceFile] {
    let fm = FileManager.default
    let root = app.directory.standardizedFileURL.resolvingSymlinksInPath()
    let hideDist = requiresBuild(app: app)
    guard
      let enumerator = fm.enumerator(
        at: root,
        includingPropertiesForKeys: [.isDirectoryKey, .isRegularFileKey, .fileSizeKey],
        options: [.skipsHiddenFiles]
      )
    else {
      return []
    }

    var files: [SourceFile] = []
    for case let url as URL in enumerator {
      let values = try url.resourceValues(forKeys: [
        .isDirectoryKey, .isRegularFileKey, .fileSizeKey,
      ])
      let relative = try relativePath(for: url, root: root)

      if values.isDirectory == true {
        if shouldSkipDirectory(
          relativePath: relative, name: url.lastPathComponent, hideDist: hideDist)
        {
          enumerator.skipDescendants()
        }
        continue
      }

      guard values.isRegularFile == true, isEditableExtension(url.pathExtension) else {
        continue
      }
      guard isUnderRoot(url, root: root) else {
        continue
      }
      if let bytes = values.fileSize, bytes > maxEditableBytes {
        continue
      }
      files.append(SourceFile(relativePath: relative, url: url.standardizedFileURL))
    }

    return files.sorted {
      $0.relativePath.localizedStandardCompare($1.relativePath) == .orderedAscending
    }
  }

  static func read(_ file: SourceFile) throws -> String {
    let values = try file.url.resourceValues(forKeys: [.fileSizeKey])
    if let bytes = values.fileSize, bytes > maxEditableBytes {
      throw SourceEditorError.fileTooLarge(file.relativePath)
    }
    let data = try Data(contentsOf: file.url)
    guard data.count <= maxEditableBytes else {
      throw SourceEditorError.fileTooLarge(file.relativePath)
    }
    guard let text = String(data: data, encoding: .utf8) else {
      throw SourceEditorError.invalidText(file.relativePath)
    }
    return text
  }

  static func write(_ text: String, to file: SourceFile, for app: TerraneApp) throws {
    let root = app.directory.standardizedFileURL.resolvingSymlinksInPath()
    guard isEditableExtension(file.url.pathExtension), isUnderRoot(file.url, root: root) else {
      throw SourceEditorError.invalidPath(file.relativePath)
    }
    guard let data = text.data(using: .utf8), data.count <= maxEditableBytes else {
      throw SourceEditorError.fileTooLarge(file.relativePath)
    }
    do {
      try data.write(to: file.url, options: .atomic)
    } catch {
      throw SourceEditorError.writeFailed(
        "Cannot write \(file.relativePath): \(error.localizedDescription)")
    }
  }

  private static func manifestObject(in appDirectory: URL) -> [String: Any]? {
    let manifest = appDirectory.appendingPathComponent("manifest.json")
    guard let data = try? Data(contentsOf: manifest),
      let object = try? JSONSerialization.jsonObject(with: data)
    else {
      return nil
    }
    return object as? [String: Any]
  }

  private static func shouldSkipDirectory(relativePath: String, name: String, hideDist: Bool)
    -> Bool
  {
    if skippedDirectories.contains(name) {
      return true
    }
    return hideDist && (relativePath == "dist" || relativePath.hasPrefix("dist/"))
  }

  private static func isEditableExtension(_ ext: String) -> Bool {
    editableExtensions.contains(ext.lowercased())
  }

  private static func relativePath(for url: URL, root: URL) throws -> String {
    let path = url.standardizedFileURL.path
    let rootPath = root.standardizedFileURL.path
    guard path.hasPrefix(rootPath + "/") else {
      throw SourceEditorError.invalidPath(path)
    }
    return String(path.dropFirst(rootPath.count + 1))
  }

  private static func isUnderRoot(_ url: URL, root: URL) -> Bool {
    let path = url.standardizedFileURL.resolvingSymlinksInPath().path
    let rootPath = root.standardizedFileURL.resolvingSymlinksInPath().path
    return path.hasPrefix(rootPath + "/")
  }
}

enum TerraneBuilder {
  static func build(appDirectory: URL) throws -> TerraneBuildResult {
    let (ok, payload) = appDirectory.path.withCString { appDirC -> (Bool, String) in
      var out: UnsafeMutablePointer<CChar>?
      var err: UnsafeMutablePointer<CChar>?
      let rc = terrane_build_app(appDirC, &out, &err)
      if rc == 0, let out {
        defer { terrane_string_free(out) }
        return (true, String(cString: out))
      }
      if let err {
        defer { terrane_string_free(err) }
        return (false, String(cString: err))
      }
      return (false, "terrane_build_app failed with code \(rc)")
    }

    guard ok else {
      throw SourceEditorError.buildFailed(payload)
    }
    guard let data = payload.data(using: .utf8),
      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let distPath = object["dist"] as? String
    else {
      throw SourceEditorError.malformedBuildOutput(payload)
    }
    let fileCount: Int
    if let intValue = object["files"] as? Int {
      fileCount = intValue
    } else if let numberValue = object["files"] as? NSNumber {
      fileCount = numberValue.intValue
    } else {
      throw SourceEditorError.malformedBuildOutput(payload)
    }
    return TerraneBuildResult(
      dist: URL(fileURLWithPath: distPath).standardizedFileURL, files: fileCount)
  }
}

enum SourceSyntaxHighlighter {
  private static let editorFont = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)

  static func apply(to textView: NSTextView, file: SourceFile?) {
    apply(to: textView, fileExtension: file?.url.pathExtension ?? "")
  }

  static func apply(to textView: NSTextView, fileExtension: String) {
    guard let storage = textView.textStorage else { return }

    let text = textView.string
    let fullRange = NSRange(location: 0, length: (text as NSString).length)
    let selectedRanges = textView.selectedRanges
    let scrollOrigin = textView.enclosingScrollView?.contentView.bounds.origin
    let baseAttributes = baseAttributes()

    storage.beginEditing()
    storage.setAttributes(baseAttributes, range: fullRange)

    switch fileExtension.lowercased() {
    case "js", "jsx", "mjs", "ts", "tsx":
      highlightJavaScript(text, fullRange: fullRange, storage: storage)
    case "json":
      highlightJSON(text, fullRange: fullRange, storage: storage)
    case "css":
      highlightCSS(text, fullRange: fullRange, storage: storage)
    case "htm", "html", "svg":
      highlightMarkup(text, fullRange: fullRange, storage: storage)
    case "md":
      highlightMarkdown(text, fullRange: fullRange, storage: storage)
    default:
      break
    }

    storage.endEditing()
    textView.typingAttributes = baseAttributes
    restoreSelection(selectedRanges, in: textView)
    if let scrollOrigin, let scrollView = textView.enclosingScrollView {
      scrollView.contentView.scroll(to: scrollOrigin)
      scrollView.reflectScrolledClipView(scrollView.contentView)
    }
  }

  private static func baseAttributes() -> [NSAttributedString.Key: Any] {
    [
      .font: editorFont,
      .foregroundColor: NSColor.textColor,
    ]
  }

  private static func highlightJavaScript(
    _ text: String, fullRange: NSRange, storage: NSTextStorage
  ) {
    applyPattern(
      #"\b(?:as|async|await|break|case|catch|class|const|continue|default|do|else|export|extends|false|finally|for|from|function|if|import|in|instanceof|interface|let|new|null|of|return|switch|throw|true|try|type|typeof|undefined|var|while)\b"#,
      color: .systemPurple,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\b\d+(?:\.\d+)?\b"#,
      color: .systemOrange,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"</?[A-Za-z][A-Za-z0-9._:-]*\b|/?>"#,
      color: .systemBlue,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #""(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`"#,
      color: .systemGreen,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"//[^\n\r]*|/\*[\s\S]*?\*/"#,
      color: .secondaryLabelColor,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
  }

  private static func highlightJSON(_ text: String, fullRange: NSRange, storage: NSTextStorage) {
    applyPattern(
      #""(?:\\.|[^"\\])*""#,
      color: .systemGreen,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #""(?:\\.|[^"\\])*"(?=\s*:)"#,
      color: .systemBlue,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\b(?:false|null|true)\b"#,
      color: .systemPurple,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b"#,
      color: .systemOrange,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
  }

  private static func highlightCSS(_ text: String, fullRange: NSRange, storage: NSTextStorage) {
    applyPattern(
      #"@[A-Za-z-]+\b"#,
      color: .systemPurple,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\b[-A-Za-z_][-_A-Za-z0-9]*(?=\s*:)"#,
      color: .systemBlue,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"#[0-9A-Fa-f]{3,8}\b"#,
      color: .systemPink,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\b\d+(?:\.\d+)?(?:%|[a-zA-Z]+)?\b"#,
      color: .systemOrange,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #""(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'"#,
      color: .systemGreen,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"/\*[\s\S]*?\*/"#,
      color: .secondaryLabelColor,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
  }

  private static func highlightMarkup(_ text: String, fullRange: NSRange, storage: NSTextStorage) {
    applyPattern(
      #"</?[A-Za-z][A-Za-z0-9:-]*\b|/?>"#,
      color: .systemBlue,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\s([A-Za-z_:][-A-Za-z0-9_:.]*)(?=\s*=)"#,
      color: .systemTeal,
      in: text,
      fullRange: fullRange,
      storage: storage
    ) { result in
      result.numberOfRanges > 1 ? result.range(at: 1) : result.range
    }
    applyPattern(
      #""(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'"#,
      color: .systemGreen,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"<!--[\s\S]*?-->"#,
      color: .secondaryLabelColor,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
  }

  private static func highlightMarkdown(_ text: String, fullRange: NSRange, storage: NSTextStorage)
  {
    applyPattern(
      #"^#{1,6}\s.+$"#,
      options: [.anchorsMatchLines],
      color: .systemBlue,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"`[^`\n]+`"#,
      color: .systemGreen,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
    applyPattern(
      #"\[[^\]]+\]\([^)]+\)"#,
      color: .systemPurple,
      in: text,
      fullRange: fullRange,
      storage: storage
    )
  }

  private static func applyPattern(
    _ pattern: String,
    options: NSRegularExpression.Options = [],
    color: NSColor,
    in text: String,
    fullRange: NSRange,
    storage: NSTextStorage,
    rangeForMatch: (NSTextCheckingResult) -> NSRange = { $0.range }
  ) {
    guard fullRange.length > 0,
      let regex = try? NSRegularExpression(pattern: pattern, options: options)
    else {
      return
    }
    for match in regex.matches(in: text, options: [], range: fullRange) {
      let range = rangeForMatch(match)
      guard isValid(range, in: fullRange) else { continue }
      storage.addAttribute(.foregroundColor, value: color, range: range)
    }
  }

  private static func isValid(_ range: NSRange, in fullRange: NSRange) -> Bool {
    range.location != NSNotFound && range.length > 0 && range.location >= fullRange.location
      && NSMaxRange(range) <= NSMaxRange(fullRange)
  }

  private static func restoreSelection(_ selectedRanges: [NSValue], in textView: NSTextView) {
    let length = textView.textStorage?.length ?? 0
    let restored = selectedRanges.compactMap { value -> NSValue? in
      let range = value.rangeValue
      guard range.location <= length else { return nil }
      let clampedLength = min(range.length, length - range.location)
      return NSValue(range: NSRange(location: range.location, length: clampedLength))
    }
    textView.selectedRanges =
      restored.isEmpty
      ? [
        NSValue(range: NSRange(location: min(textView.selectedRange().location, length), length: 0))
      ]
      : restored
  }
}

struct SourceEditorSaveResult {
  let message: String
}

final class SourceEditorPanel: NSView, NSTextViewDelegate {
  var onSave: ((TerraneApp, SourceFile, String) throws -> SourceEditorSaveResult)?

  private let title = NSTextField(labelWithString: "Code")
  private let fileMenu = NSPopUpButton()
  private let textView = NSTextView()
  private let scrollView = NSScrollView()
  private let saveButton = NSButton(title: "Save & Reload", target: nil, action: nil)
  private let status = NSTextField(labelWithString: "")

  private var app: TerraneApp?
  private var files: [SourceFile] = []
  private var selectedFile: SourceFile?
  private var highlightWorkItem: DispatchWorkItem?
  private var isDirty = false

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    buildView()
  }

  required init?(coder: NSCoder) {
    super.init(coder: coder)
    buildView()
  }

  func setApp(_ app: TerraneApp?, preferredPath: String? = nil) {
    highlightWorkItem?.cancel()
    self.app = app
    selectedFile = nil
    isDirty = false
    fileMenu.removeAllItems()

    guard let app else {
      files = []
      textView.string = ""
      status.stringValue = "No app selected."
      updateControls()
      return
    }

    do {
      files = try SourceEditorModel.editableFiles(for: app)
      for file in files {
        fileMenu.addItem(withTitle: file.relativePath)
      }
      if let index = preferredPath.flatMap({ path in files.firstIndex { $0.relativePath == path } })
        ?? files.indices.first
      {
        selectedFile = files[index]
        fileMenu.selectItem(at: index)
        loadSelectedFile()
      } else {
        textView.string = ""
        status.stringValue = "No editable text files in \(app.name)."
      }
    } catch {
      files = []
      textView.string = ""
      status.stringValue = error.localizedDescription
    }
    updateControls()
  }

  func confirmDiscardIfNeeded(window: NSWindow?) -> Bool {
    guard isDirty else { return true }
    _ = window
    let alert = NSAlert()
    alert.messageText = "Discard unsaved changes?"
    alert.informativeText = "The current file has edits that have not been saved."
    alert.addButton(withTitle: "Discard")
    alert.addButton(withTitle: "Cancel")
    return alert.runModal() == .alertFirstButtonReturn
  }

  private func buildView() {
    wantsLayer = true
    layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

    title.font = .systemFont(ofSize: 13, weight: .semibold)
    title.translatesAutoresizingMaskIntoConstraints = false

    fileMenu.target = self
    fileMenu.action = #selector(fileChanged(_:))
    fileMenu.translatesAutoresizingMaskIntoConstraints = false

    textView.isRichText = false
    textView.isAutomaticQuoteSubstitutionEnabled = false
    textView.isAutomaticDashSubstitutionEnabled = false
    textView.drawsBackground = true
    textView.backgroundColor = .textBackgroundColor
    textView.textColor = .textColor
    textView.insertionPointColor = .textColor
    textView.textContainerInset = NSSize(width: 8, height: 8)
    textView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
    textView.frame = NSRect(x: 0, y: 0, width: 360, height: 600)
    textView.minSize = NSSize(width: 0, height: 0)
    textView.maxSize = NSSize(
      width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    textView.isVerticallyResizable = true
    textView.isHorizontallyResizable = false
    textView.autoresizingMask = [.width]
    textView.textContainer?.containerSize = NSSize(
      width: 360, height: CGFloat.greatestFiniteMagnitude)
    textView.textContainer?.widthTracksTextView = true
    textView.delegate = self

    scrollView.borderType = .bezelBorder
    scrollView.drawsBackground = true
    scrollView.backgroundColor = .textBackgroundColor
    scrollView.hasVerticalScroller = true
    scrollView.hasHorizontalScroller = false
    scrollView.documentView = textView
    scrollView.translatesAutoresizingMaskIntoConstraints = false

    saveButton.target = self
    saveButton.action = #selector(saveCurrentFile(_:))
    saveButton.bezelStyle = .rounded
    saveButton.translatesAutoresizingMaskIntoConstraints = false

    status.textColor = .secondaryLabelColor
    status.lineBreakMode = .byTruncatingTail
    status.translatesAutoresizingMaskIntoConstraints = false

    addSubview(title)
    addSubview(fileMenu)
    addSubview(scrollView)
    addSubview(saveButton)
    addSubview(status)

    NSLayoutConstraint.activate([
      title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
      title.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
      title.topAnchor.constraint(equalTo: topAnchor, constant: 12),

      fileMenu.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
      fileMenu.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
      fileMenu.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 8),

      scrollView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
      scrollView.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
      scrollView.topAnchor.constraint(equalTo: fileMenu.bottomAnchor, constant: 10),
      scrollView.bottomAnchor.constraint(equalTo: saveButton.topAnchor, constant: -10),

      saveButton.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
      saveButton.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),

      status.leadingAnchor.constraint(equalTo: saveButton.trailingAnchor, constant: 10),
      status.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
      status.centerYAnchor.constraint(equalTo: saveButton.centerYAnchor),
    ])
    updateControls()
  }

  private func loadSelectedFile() {
    highlightWorkItem?.cancel()
    guard let file = selectedFile else { return }
    do {
      textView.string = try SourceEditorModel.read(file)
      highlightCurrentFile()
      isDirty = false
      status.stringValue = file.relativePath
      resetEditorViewport()
    } catch {
      textView.string = ""
      isDirty = false
      status.stringValue = error.localizedDescription
      resetEditorViewport()
    }
    updateControls()
  }

  private func highlightCurrentFile() {
    highlightWorkItem = nil
    SourceSyntaxHighlighter.apply(to: textView, file: selectedFile)
  }

  private func scheduleSyntaxHighlight() {
    highlightWorkItem?.cancel()
    let item = DispatchWorkItem { [weak self] in
      self?.highlightCurrentFile()
    }
    highlightWorkItem = item
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: item)
  }

  private func resetEditorViewport() {
    textView.setSelectedRange(NSRange(location: 0, length: 0))
    textView.scrollRangeToVisible(NSRange(location: 0, length: 0))
    let currentOrigin = scrollView.contentView.bounds.origin
    scrollView.contentView.scroll(to: NSPoint(x: 0, y: currentOrigin.y))
    scrollView.reflectScrolledClipView(scrollView.contentView)
  }

  private func updateControls() {
    let hasFile = selectedFile != nil
    fileMenu.isEnabled = !files.isEmpty
    textView.isEditable = hasFile
    saveButton.isEnabled = hasFile && isDirty
  }

  @objc private func fileChanged(_ sender: NSPopUpButton) {
    guard files.indices.contains(sender.indexOfSelectedItem) else { return }
    if !confirmDiscardIfNeeded(window: window) {
      if let current = selectedFile,
        let index = files.firstIndex(of: current)
      {
        fileMenu.selectItem(at: index)
      }
      return
    }
    selectedFile = files[sender.indexOfSelectedItem]
    loadSelectedFile()
  }

  @objc private func saveCurrentFile(_ sender: NSButton) {
    guard let app, let file = selectedFile else { return }
    do {
      let result = try onSave?(app, file, textView.string)
      isDirty = false
      status.stringValue = result?.message ?? "Saved."
    } catch {
      status.stringValue = error.localizedDescription
    }
    updateControls()
  }

  func textDidChange(_ notification: Notification) {
    isDirty = true
    status.stringValue = "Unsaved changes"
    scheduleSyntaxHighlight()
    updateControls()
  }
}
