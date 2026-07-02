import AppKit

/// Sidebar footer for the local-model edge: shows whether the MLX runtime and
/// its resident server are available, offers one-click setup (downloads the
/// runtime on machines without one), and lets the user stop the resident
/// server. All state lives behind the Terrane C ABI
/// (`terrane_local_model_*`); this view is presentation only.
final class LocalModelPanel: NSView {
  private let caption = NSTextField(labelWithString: "Local models")
  private let statusLabel = NSTextField(labelWithString: "…")
  private let actionButton = NSButton(title: "", target: nil, action: nil)
  private let spinner = NSProgressIndicator()

  private var homePath: String = ""
  private var runtimeAvailable = false
  private var serverRunning = false
  private var busy = false

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    configure()
  }

  required init?(coder: NSCoder) {
    super.init(coder: coder)
    configure()
  }

  /// Point the panel at a workspace and load current state.
  func configure(home: URL) {
    homePath = home.path
    refresh()
  }

  /// Re-query runtime + server state off the main thread and re-render.
  func refresh() {
    guard !homePath.isEmpty, !busy else { return }
    let home = homePath
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      let status = LocalModelPanel.fetchStatus(home: home)
      DispatchQueue.main.async {
        guard let self else { return }
        self.runtimeAvailable = status.runtimeAvailable
        self.serverRunning = status.running
        self.render()
      }
    }
  }

  private func render() {
    spinner.isHidden = !busy
    actionButton.isEnabled = !busy
    if busy {
      statusLabel.stringValue = "Installing runtime…"
      actionButton.title = "Installing…"
      return
    }
    if !runtimeAvailable {
      statusLabel.stringValue = "Apple acceleration not set up"
      actionButton.title = "Set Up MLX…"
      actionButton.isHidden = false
    } else if serverRunning {
      statusLabel.stringValue = "MLX server running"
      actionButton.title = "Stop Server"
      actionButton.isHidden = false
    } else {
      statusLabel.stringValue = "MLX ready — starts on demand"
      actionButton.isHidden = true
    }
  }

  @objc private func performAction() {
    if !runtimeAvailable {
      runSetup()
    } else if serverRunning {
      runStop()
    }
  }

  private func runSetup() {
    let alert = NSAlert()
    alert.messageText = "Set up Apple acceleration?"
    alert.informativeText =
      "Terrane will install the MLX runtime (mlx-lm) into this workspace's engines folder. "
      + "On a machine without the runtime this downloads a few hundred MB once."
    alert.addButton(withTitle: "Install")
    alert.addButton(withTitle: "Cancel")
    guard alert.runModal() == .alertFirstButtonReturn else { return }

    busy = true
    spinner.startAnimation(nil)
    render()
    let home = homePath
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      let result = LocalModelPanel.callStringOut(home: home) {
        terrane_local_model_setup_mlx($0, $1, $2)
      }
      DispatchQueue.main.async {
        guard let self else { return }
        self.busy = false
        self.spinner.stopAnimation(nil)
        switch result {
        case .success(let summary):
          self.presentInfo(title: "MLX runtime ready", text: summary)
        case .failure(let message):
          self.presentInfo(title: "MLX setup failed", text: message)
        }
        self.refresh()
      }
    }
  }

  private func runStop() {
    let home = homePath
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      _ = LocalModelPanel.callStringOut(home: home) {
        terrane_local_model_server_stop($0, $1, $2)
      }
      DispatchQueue.main.async { self?.refresh() }
    }
  }

  private func presentInfo(title: String, text: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = text
    alert.runModal()
  }

  // MARK: C ABI plumbing

  private struct Status {
    var running = false
    var runtimeAvailable = false
  }

  private static func fetchStatus(home: String) -> Status {
    switch callStringOut(home: home, { terrane_local_model_server_status($0, $1, $2) }) {
    case .failure:
      return Status()
    case .success(let json):
      guard
        let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
      else {
        return Status()
      }
      return Status(
        running: object["running"] as? Bool ?? false,
        runtimeAvailable: object["runtimeAvailable"] as? Bool ?? false
      )
    }
  }

  private enum CallResult {
    case success(String)
    case failure(String)
  }

  /// Run one `terrane_local_model_*` export, owning the returned C strings.
  private static func callStringOut(
    home: String,
    _ call: (
      UnsafePointer<CChar>?,
      UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?,
      UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
    ) -> Int32
  ) -> CallResult {
    var out: UnsafeMutablePointer<CChar>? = nil
    var err: UnsafeMutablePointer<CChar>? = nil
    let code = home.withCString { call($0, &out, &err) }
    defer {
      terrane_string_free(out)
      terrane_string_free(err)
    }
    if code == TERRANE_OK, let out {
      return .success(String(cString: out))
    }
    if let err {
      return .failure(String(cString: err))
    }
    return .failure("terrane local-model call failed with code \(code)")
  }

  // MARK: layout

  private func configure() {
    wantsLayer = true
    layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.35).cgColor
    layer?.cornerRadius = 10

    caption.font = .systemFont(ofSize: 11, weight: .medium)
    caption.textColor = .secondaryLabelColor
    caption.translatesAutoresizingMaskIntoConstraints = false

    statusLabel.font = .systemFont(ofSize: 12)
    statusLabel.textColor = .labelColor
    statusLabel.lineBreakMode = .byTruncatingTail
    statusLabel.translatesAutoresizingMaskIntoConstraints = false

    actionButton.bezelStyle = .rounded
    actionButton.controlSize = .small
    actionButton.font = .systemFont(ofSize: 11, weight: .medium)
    actionButton.target = self
    actionButton.action = #selector(performAction)
    actionButton.translatesAutoresizingMaskIntoConstraints = false

    spinner.style = .spinning
    spinner.controlSize = .small
    spinner.isHidden = true
    spinner.translatesAutoresizingMaskIntoConstraints = false

    addSubview(caption)
    addSubview(statusLabel)
    addSubview(actionButton)
    addSubview(spinner)

    NSLayoutConstraint.activate([
      caption.topAnchor.constraint(equalTo: topAnchor, constant: 8),
      caption.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      caption.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),

      statusLabel.topAnchor.constraint(equalTo: caption.bottomAnchor, constant: 4),
      statusLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      statusLabel.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),

      actionButton.topAnchor.constraint(equalTo: statusLabel.bottomAnchor, constant: 8),
      actionButton.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      actionButton.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),

      spinner.centerYAnchor.constraint(equalTo: actionButton.centerYAnchor),
      spinner.leadingAnchor.constraint(equalTo: actionButton.trailingAnchor, constant: 8),
    ])
  }
}
