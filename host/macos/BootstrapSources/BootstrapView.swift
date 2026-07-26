import AppKit

final class BootstrapProgressView: NSView {
  var progress: Double? {
    didSet {
      setAccessibilityValue(progress.map { NSNumber(value: $0 * 100) })
      setAccessibilityValueDescription(
        progress.map { "\(Int(($0 * 100).rounded())) percent" } ?? "Working")
      needsDisplay = true
    }
  }

  private var animationPhase: CGFloat = 0
  private var timer: Timer?

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    wantsLayer = true
    setAccessibilityElement(true)
    setAccessibilityRole(.progressIndicator)
    setAccessibilityLabel("Terrane download progress")
    setAccessibilityMinValue(NSNumber(value: 0))
    setAccessibilityMaxValue(NSNumber(value: 100))
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) has not been implemented")
  }

  override var intrinsicContentSize: NSSize {
    NSSize(width: NSView.noIntrinsicMetric, height: 12)
  }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    if window != nil {
      timer = Timer.scheduledTimer(withTimeInterval: 1 / 30, repeats: true) {
        [weak self] _ in
        guard let self, self.progress == nil else { return }
        self.animationPhase = (self.animationPhase + 0.018).truncatingRemainder(dividingBy: 1)
        self.needsDisplay = true
      }
    } else {
      timer?.invalidate()
      timer = nil
    }
  }

  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    let track = bounds.insetBy(dx: 0, dy: 1)
    let radius = track.height / 2
    NSColor.quaternaryLabelColor.withAlphaComponent(0.45).setFill()
    NSBezierPath(roundedRect: track, xRadius: radius, yRadius: radius).fill()

    let fillRect: NSRect
    if let progress {
      fillRect = NSRect(
        x: track.minX,
        y: track.minY,
        width: max(track.height, track.width * min(1, max(0, progress))),
        height: track.height
      )
    } else {
      let segment = track.width * 0.28
      let travel = track.width + segment
      fillRect = NSRect(
        x: track.minX + travel * animationPhase - segment,
        y: track.minY,
        width: segment,
        height: track.height
      ).intersection(track)
    }
    guard !fillRect.isEmpty else { return }
    NSGraphicsContext.saveGraphicsState()
    NSBezierPath(roundedRect: track, xRadius: radius, yRadius: radius).addClip()
    let gradient = NSGradient(colors: [
      NSColor(calibratedRed: 0.13, green: 0.56, blue: 0.46, alpha: 1),
      NSColor(calibratedRed: 0.31, green: 0.76, blue: 0.62, alpha: 1),
    ])
    gradient?.draw(in: fillRect, angle: 0)
    NSGraphicsContext.restoreGraphicsState()
  }
}

final class BootstrapLogoView: NSView {
  override var intrinsicContentSize: NSSize { NSSize(width: 76, height: 76) }

  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    let tile = bounds.insetBy(dx: 3, dy: 3)
    let background = NSBezierPath(roundedRect: tile, xRadius: 19, yRadius: 19)
    NSColor(calibratedRed: 0.075, green: 0.16, blue: 0.14, alpha: 1).setFill()
    background.fill()

    // The crossbar deliberately reaches outside the stem's visual bounds,
    // giving the T the organic, slightly unruly silhouette used by Terrane.
    let mark = NSBezierPath()
    mark.move(to: NSPoint(x: tile.minX + 11, y: tile.maxY - 23))
    mark.curve(
      to: NSPoint(x: tile.maxX - 8, y: tile.maxY - 18),
      controlPoint1: NSPoint(x: tile.minX + 23, y: tile.maxY - 32),
      controlPoint2: NSPoint(x: tile.maxX - 25, y: tile.maxY - 10)
    )
    mark.line(to: NSPoint(x: tile.maxX - 12, y: tile.maxY - 31))
    mark.curve(
      to: NSPoint(x: tile.midX + 7, y: tile.maxY - 31),
      controlPoint1: NSPoint(x: tile.maxX - 26, y: tile.maxY - 27),
      controlPoint2: NSPoint(x: tile.midX + 13, y: tile.maxY - 31)
    )
    mark.curve(
      to: NSPoint(x: tile.midX + 2, y: tile.minY + 12),
      controlPoint1: NSPoint(x: tile.midX + 8, y: tile.midY + 7),
      controlPoint2: NSPoint(x: tile.midX + 12, y: tile.minY + 20)
    )
    mark.line(to: NSPoint(x: tile.midX - 12, y: tile.minY + 15))
    mark.curve(
      to: NSPoint(x: tile.midX - 6, y: tile.maxY - 31),
      controlPoint1: NSPoint(x: tile.midX - 4, y: tile.minY + 29),
      controlPoint2: NSPoint(x: tile.midX - 8, y: tile.midY + 12)
    )
    mark.curve(
      to: NSPoint(x: tile.minX + 14, y: tile.maxY - 34),
      controlPoint1: NSPoint(x: tile.midX - 17, y: tile.maxY - 31),
      controlPoint2: NSPoint(x: tile.minX + 23, y: tile.maxY - 31)
    )
    mark.close()
    NSColor(calibratedRed: 0.50, green: 0.92, blue: 0.74, alpha: 1).setFill()
    mark.fill()
  }
}

final class BootstrapViewController: NSViewController, BootstrapManagerDelegate {
  private let manager: BootstrapManager
  private let titleLabel = NSTextField(labelWithString: "Checking Terrane")
  private let detailLabel = NSTextField(wrappingLabelWithString: "")
  private let byteLabel = NSTextField(labelWithString: "")
  private let progressView = BootstrapProgressView()
  private let retryButton = NSButton(title: "Try Again", target: nil, action: nil)

  init(manager: BootstrapManager) {
    self.manager = manager
    super.init(nibName: nil, bundle: nil)
    manager.delegate = self
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) has not been implemented")
  }

  override func loadView() {
    let root = NSView()
    root.translatesAutoresizingMaskIntoConstraints = false
    root.wantsLayer = true
    root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

    let logo = BootstrapLogoView()
    logo.translatesAutoresizingMaskIntoConstraints = false

    titleLabel.font = .systemFont(ofSize: 22, weight: .semibold)
    titleLabel.alignment = .center
    titleLabel.maximumNumberOfLines = 1

    detailLabel.font = .systemFont(ofSize: 13)
    detailLabel.textColor = .secondaryLabelColor
    detailLabel.alignment = .center
    detailLabel.maximumNumberOfLines = 3
    detailLabel.preferredMaxLayoutWidth = 390

    byteLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .medium)
    byteLabel.textColor = .tertiaryLabelColor
    byteLabel.alignment = .right

    progressView.translatesAutoresizingMaskIntoConstraints = false
    retryButton.target = self
    retryButton.action = #selector(retry)
    retryButton.bezelStyle = .rounded
    retryButton.controlSize = .large
    retryButton.isHidden = true

    let progressStack = NSStackView(views: [progressView, byteLabel])
    progressStack.orientation = .vertical
    progressStack.spacing = 8
    progressStack.alignment = .width

    let stack = NSStackView(views: [logo, titleLabel, detailLabel, progressStack, retryButton])
    stack.translatesAutoresizingMaskIntoConstraints = false
    stack.orientation = .vertical
    stack.alignment = .centerX
    stack.spacing = 15
    stack.setCustomSpacing(22, after: logo)
    stack.setCustomSpacing(24, after: detailLabel)
    root.addSubview(stack)

    NSLayoutConstraint.activate([
      root.widthAnchor.constraint(equalToConstant: 520),
      root.heightAnchor.constraint(equalToConstant: 360),
      stack.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 54),
      stack.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -54),
      stack.centerYAnchor.constraint(equalTo: root.centerYAnchor, constant: -4),
      logo.widthAnchor.constraint(equalToConstant: 76),
      logo.heightAnchor.constraint(equalToConstant: 76),
      progressView.widthAnchor.constraint(equalTo: stack.widthAnchor),
      retryButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 120),
    ])
    view = root
  }

  override func viewDidAppear() {
    super.viewDidAppear()
    manager.start()
  }

  func bootstrapManager(_ manager: BootstrapManager, didUpdate state: BootstrapViewState) {
    titleLabel.stringValue = state.title
    detailLabel.stringValue = state.detail
    byteLabel.stringValue = state.byteDetail ?? ""
    byteLabel.isHidden = state.byteDetail == nil
    progressView.progress = state.progress
    progressView.isHidden = state.phase == .failed
    retryButton.isHidden = !state.retryAvailable
    view.window?.setAccessibilityTitle("\(state.title). \(state.detail)")
  }

  func bootstrapManagerDidComplete(_ manager: BootstrapManager) {
    NSApp.terminate(nil)
  }

  @objc private func retry() {
    retryButton.isHidden = true
    progressView.isHidden = false
    manager.retry()
  }
}
