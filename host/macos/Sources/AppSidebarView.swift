import AppKit

final class AppSidebarView: NSVisualEffectView {
  var onSelect: ((TerraneApp) -> Void)?
  var onSelectPremium: ((PremiumApp) -> Void)?
  var onHome: (() -> Void)?
  var onToggleCollapse: (() -> Void)?

  private let collapseButton = NSButton()
  private let brandIcon = NSImageView()
  private let title = NSTextField(labelWithString: "Terrane")
  private let caption = NSTextField(labelWithString: "Apps")
  private let premiumCaption = NSTextField(labelWithString: "Premium")
  private let stack = NSStackView()
  private let homeButton = AppSidebarButton(
    title: "Home",
    appId: "",
    iconImage: NSImage(systemSymbolName: "house", accessibilityDescription: nil)
  )
  let localModelPanel = LocalModelPanel()
  private var apps: [TerraneApp] = []
  private var premiumApps: [PremiumApp] = []
  private var selectedAppId: String?
  private var buttons: [AppSidebarButton] = []
  private var premiumButtons: [AppSidebarButton] = []
  private var isCollapsed = false

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    configure()
  }

  required init?(coder: NSCoder) {
    super.init(coder: coder)
    configure()
  }

  func render(apps: [TerraneApp], premiumApps: [PremiumApp] = [], selectedAppId: String?) {
    self.apps = apps
    self.premiumApps = premiumApps.filter { premium in
      !apps.contains { $0.id == premium.id }
    }
    self.selectedAppId = selectedAppId

    buttons.forEach { $0.removeFromSuperview() }
    buttons = []
    premiumButtons.forEach { $0.removeFromSuperview() }
    premiumButtons = []
    premiumCaption.removeFromSuperview()
    homeButton.isSelected = selectedAppId == nil

    for (index, app) in apps.enumerated() {
      let button = AppSidebarButton(
        title: app.name,
        appId: app.id,
        iconImage: Self.iconImage(for: app),
        target: self,
        action: #selector(selectApp(_:))
      )
      button.tag = index
      button.isSelected = app.id == selectedAppId
      button.setCollapsed(isCollapsed)
      buttons.append(button)
      stack.addArrangedSubview(button)
    }

    guard !self.premiumApps.isEmpty else { return }
    premiumCaption.isHidden = isCollapsed
    stack.addArrangedSubview(premiumCaption)
    for (index, app) in self.premiumApps.enumerated() {
      let button = AppSidebarButton(
        title: app.name,
        appId: app.id,
        iconImage: Self.iconImage(for: app),
        target: self,
        action: #selector(selectPremiumApp(_:))
      )
      button.tag = index
      button.setCollapsed(isCollapsed)
      premiumButtons.append(button)
      stack.addArrangedSubview(button)
    }
  }

  func setCollapsed(_ collapsed: Bool) {
    isCollapsed = collapsed
    brandIcon.isHidden = collapsed
    title.isHidden = collapsed
    caption.isHidden = collapsed
    premiumCaption.isHidden = collapsed || premiumApps.isEmpty
    localModelPanel.isHidden = collapsed
    collapseButton.image = NSImage(
      systemSymbolName: collapsed ? "sidebar.right" : "sidebar.left",
      accessibilityDescription: nil
    )
    collapseButton.toolTip = collapsed ? "Expand apps" : "Collapse apps"
    collapseButton.state = collapsed ? .on : .off
    stack.spacing = collapsed ? 10 : 6
    homeButton.setCollapsed(collapsed)
    for button in buttons {
      button.setCollapsed(collapsed)
    }
    for button in premiumButtons {
      button.setCollapsed(collapsed)
    }
  }

  func select(appId: String?) {
    selectedAppId = appId
    homeButton.isSelected = appId == nil
    for (index, button) in buttons.enumerated() {
      button.isSelected = apps.indices.contains(index) && apps[index].id == appId
    }
  }

  func selectApp(at index: Int) {
    guard apps.indices.contains(index) else { return }
    onSelect?(apps[index])
  }

  private func configure() {
    material = .sidebar
    blendingMode = .withinWindow
    state = .active
    wantsLayer = true
    layer?.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.38).cgColor

    brandIcon.image = NSImage(systemSymbolName: "apps.iphone", accessibilityDescription: nil)
    brandIcon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 18, weight: .semibold)
    brandIcon.contentTintColor = .systemGreen
    brandIcon.wantsLayer = true
    brandIcon.layer?.cornerRadius = 9
    brandIcon.layer?.borderWidth = 1
    brandIcon.layer?.borderColor = NSColor.separatorColor.withAlphaComponent(0.38).cgColor
    brandIcon.layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.45).cgColor
    brandIcon.translatesAutoresizingMaskIntoConstraints = false

    title.font = .systemFont(ofSize: 15, weight: .semibold)
    title.textColor = .labelColor
    title.translatesAutoresizingMaskIntoConstraints = false

    caption.font = .systemFont(ofSize: 11, weight: .medium)
    caption.textColor = .secondaryLabelColor
    caption.translatesAutoresizingMaskIntoConstraints = false
    premiumCaption.font = .systemFont(ofSize: 11, weight: .medium)
    premiumCaption.textColor = .secondaryLabelColor
    premiumCaption.translatesAutoresizingMaskIntoConstraints = false

    collapseButton.image = NSImage(systemSymbolName: "sidebar.left", accessibilityDescription: nil)
    collapseButton.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 16, weight: .medium)
    collapseButton.bezelStyle = .regularSquare
    collapseButton.setButtonType(.toggle)
    collapseButton.isBordered = false
    collapseButton.target = self
    collapseButton.action = #selector(toggleCollapse)
    collapseButton.toolTip = "Collapse apps"
    collapseButton.contentTintColor = .secondaryLabelColor
    collapseButton.wantsLayer = true
    collapseButton.layer?.cornerRadius = 9
    collapseButton.layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.5).cgColor
    collapseButton.translatesAutoresizingMaskIntoConstraints = false

    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 4
    stack.translatesAutoresizingMaskIntoConstraints = false

    homeButton.target = self
    homeButton.action = #selector(goHome)
    homeButton.isSelected = true
    stack.addArrangedSubview(homeButton)

    localModelPanel.translatesAutoresizingMaskIntoConstraints = false

    addSubview(brandIcon)
    addSubview(collapseButton)
    addSubview(title)
    addSubview(caption)
    addSubview(stack)
    addSubview(localModelPanel)

    NSLayoutConstraint.activate([
      localModelPanel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      localModelPanel.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
      localModelPanel.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12),
    ])

    NSLayoutConstraint.activate([
      brandIcon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
      brandIcon.topAnchor.constraint(equalTo: topAnchor, constant: 42),
      brandIcon.widthAnchor.constraint(equalToConstant: 34),
      brandIcon.heightAnchor.constraint(equalToConstant: 34),

      collapseButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
      collapseButton.centerYAnchor.constraint(equalTo: brandIcon.centerYAnchor),
      collapseButton.widthAnchor.constraint(equalToConstant: 34),
      collapseButton.heightAnchor.constraint(equalToConstant: 34),

      title.leadingAnchor.constraint(equalTo: brandIcon.trailingAnchor, constant: 12),
      title.trailingAnchor.constraint(lessThanOrEqualTo: collapseButton.leadingAnchor, constant: -10),
      title.centerYAnchor.constraint(equalTo: brandIcon.centerYAnchor),

      caption.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
      caption.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
      caption.topAnchor.constraint(equalTo: brandIcon.bottomAnchor, constant: 22),

      stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
      stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
      stack.topAnchor.constraint(equalTo: caption.bottomAnchor, constant: 12),
    ])
  }

  @objc private func selectApp(_ sender: NSButton) {
    selectApp(at: sender.tag)
  }

  @objc private func selectPremiumApp(_ sender: NSButton) {
    guard premiumApps.indices.contains(sender.tag) else { return }
    onSelectPremium?(premiumApps[sender.tag])
  }

  @objc private func goHome(_ sender: NSButton) {
    onHome?()
  }

  @objc private func toggleCollapse(_ sender: NSButton) {
    onToggleCollapse?()
  }

  static func iconImage(for app: TerraneApp) -> NSImage? {
    if let iconURL = app.iconURL, let image = NSImage(contentsOf: iconURL) {
      image.isTemplate = true
      return image
    }
    return NSImage(systemSymbolName: "app.dashed", accessibilityDescription: nil)
  }

  static func iconImage(for app: PremiumApp) -> NSImage? {
    if app.icon == "checklist" || app.id.contains("todo") {
      return NSImage(systemSymbolName: "checklist", accessibilityDescription: nil)
    }
    if app.id.contains("shop") {
      return NSImage(systemSymbolName: "bag", accessibilityDescription: nil)
    }
    if app.id.contains("admin") {
      return NSImage(systemSymbolName: "shield", accessibilityDescription: nil)
    }
    if app.id.contains("studio") {
      return NSImage(systemSymbolName: "pencil", accessibilityDescription: nil)
    }
    return NSImage(systemSymbolName: "sparkles", accessibilityDescription: nil)
  }
}

final class AppSidebarButton: NSButton {
  private let fullTitle: String
  private let appId: String
  private let iconImage: NSImage?
  private var widthConstraint: NSLayoutConstraint?
  private var heightConstraint: NSLayoutConstraint?
  private var collapsed = false

  var isSelected = false {
    didSet {
      needsDisplay = true
      contentTintColor = isSelected ? .labelColor : .secondaryLabelColor
    }
  }

  override init(frame frameRect: NSRect) {
    fullTitle = ""
    appId = ""
    iconImage = NSImage(systemSymbolName: "app.dashed", accessibilityDescription: nil)
    super.init(frame: frameRect)
    configure()
  }

  convenience init(
    title: String,
    appId: String,
    iconImage: NSImage?,
    target: AnyObject?,
    action: Selector?
  ) {
    self.init(title: title, appId: appId, iconImage: iconImage)
    self.target = target
    self.action = action
  }

  init(title: String, appId: String, iconImage: NSImage?) {
    fullTitle = title
    self.appId = appId
    self.iconImage = iconImage
    super.init(frame: .zero)
    self.title = title
    toolTip = appId
    configure()
  }

  required init?(coder: NSCoder) {
    fullTitle = ""
    appId = ""
    iconImage = NSImage(systemSymbolName: "app.dashed", accessibilityDescription: nil)
    super.init(coder: coder)
    configure()
  }

  override var intrinsicContentSize: NSSize {
    NSSize(width: NSView.noIntrinsicMetric, height: collapsed ? 52 : 38)
  }

  func setCollapsed(_ collapsed: Bool) {
    self.collapsed = collapsed
    title = collapsed ? "" : fullTitle
    alignment = collapsed ? .center : .left
    imageHugsTitle = !collapsed
    widthConstraint?.constant = collapsed ? 56 : 212
    heightConstraint?.constant = collapsed ? 52 : 38
    toolTip = collapsed ? fullTitle : appId
    needsLayout = true
    invalidateIntrinsicContentSize()
  }

  override func draw(_ dirtyRect: NSRect) {
    if isSelected {
      let selectedRect = bounds.insetBy(dx: collapsed ? 3 : 0, dy: collapsed ? 3 : 1)
      NSColor.labelColor.withAlphaComponent(0.1).setFill()
      NSBezierPath(roundedRect: selectedRect, xRadius: 12, yRadius: 12).fill()

      if !collapsed {
        NSColor.systemGreen.withAlphaComponent(0.8).setFill()
        let marker = NSRect(x: 5, y: selectedRect.midY - 9, width: 3, height: 18)
        NSBezierPath(roundedRect: marker, xRadius: 1.5, yRadius: 1.5).fill()
      }
    }
    super.draw(dirtyRect)
  }

  private func configure() {
    bezelStyle = .regularSquare
    isBordered = false
    alignment = .left
    font = .systemFont(ofSize: 14, weight: .medium)
    iconImage?.isTemplate = true
    image = iconImage
    symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 16, weight: .semibold)
    imagePosition = .imageLeading
    imageHugsTitle = true
    setButtonType(.momentaryChange)
    lineBreakMode = .byTruncatingTail
    contentTintColor = .secondaryLabelColor
    translatesAutoresizingMaskIntoConstraints = false
    widthConstraint = widthAnchor.constraint(equalToConstant: 212)
    heightConstraint = heightAnchor.constraint(equalToConstant: 38)
    widthConstraint?.isActive = true
    heightConstraint?.isActive = true
  }
}
