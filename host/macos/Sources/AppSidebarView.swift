import AppKit

struct AppSidebarSectionItem: Equatable {
  let id: String
  let title: String
  let subtitle: String?
  let systemImage: String?
}

struct AppSidebarSection: Equatable {
  let title: String
  let items: [AppSidebarSectionItem]
  let selectedItemId: String?
  let createLabel: String?
}

/// Native host-owned sidebar chrome. The split, resizing, collapse controls,
/// selection presentation, and app-switch lifecycle live here; apps only
/// publish an AppSidebarSection and respond to select/create callbacks.
final class AppSidebarView: NSVisualEffectView, NSSplitViewDelegate {
  var onSelect: ((TerraneApp) -> Void)?
  var onSelectPremium: ((PremiumApp) -> Void)?
  var onHome: (() -> Void)?
  var onToggleCollapse: (() -> Void)?
  var onSectionItemSelect: ((String) -> Void)?
  var onSectionCreate: (() -> Void)?

  private let collapseButton = NSButton()
  private let brandIcon = NSImageView()
  private let title = NSTextField(labelWithString: "Terrane")
  private let splitView = NSSplitView()
  private let appsPane = NSView()
  private let appsHeader = NSButton()
  private let appsScroll = NSScrollView()
  private let premiumCaption = NSTextField(labelWithString: "Premium")
  private let stack = NSStackView()
  private let detailPane = AppProvidedSidebarView()
  private let homeButton = AppSidebarButton(
    title: "Home",
    appId: "",
    iconImage: NSImage(systemSymbolName: "house", accessibilityDescription: nil)
  )
  private var apps: [TerraneApp] = []
  private var premiumApps: [PremiumApp] = []
  private var selectedAppId: String?
  private var buttons: [AppSidebarButton] = []
  private var premiumButtons: [AppSidebarButton] = []
  private var isCollapsed = false
  private var appsSectionCollapsed = false
  private var detailSectionCollapsed = false
  private var savedDividerPosition: CGFloat = 360

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    configure()
  }

  required init?(coder: NSCoder) {
    super.init(coder: coder)
    configure()
  }

  override func layout() {
    super.layout()
    guard splitView.bounds.height > 100 else { return }
    splitView.layoutSubtreeIfNeeded()
    if isCollapsed || detailPane.section == nil {
      if appsPane.frame.height < splitView.bounds.height - 2 {
        splitView.setPosition(splitView.bounds.height, ofDividerAt: 0)
      }
      return
    }
    if appsSectionCollapsed {
      splitView.setPosition(36, ofDividerAt: 0)
      return
    }
    if detailSectionCollapsed {
      splitView.setPosition(max(36, splitView.bounds.height - 36), ofDividerAt: 0)
      return
    }
    if appsPane.frame.height < 42 || detailPane.frame.height < 42 {
      let maximum = max(150, splitView.bounds.height - 118)
      splitView.setPosition(min(savedDividerPosition, maximum), ofDividerAt: 0)
    }
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

    if !self.premiumApps.isEmpty {
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
    resetAppsScrollToTop()
  }

  func setAppSection(_ section: AppSidebarSection?) {
    detailPane.render(section)
    if section != nil, !isCollapsed {
      restoreDividerIfNeeded()
    } else {
      splitView.setPosition(splitView.bounds.height, ofDividerAt: 0)
    }
  }

  func setCollapsed(_ collapsed: Bool) {
    isCollapsed = collapsed
    brandIcon.isHidden = collapsed
    title.isHidden = collapsed
    appsHeader.isHidden = collapsed
    premiumCaption.isHidden = collapsed || premiumApps.isEmpty
    if collapsed {
      detailPane.setHostVisible(false)
    }
    collapseButton.image = NSImage(
      systemSymbolName: collapsed ? "sidebar.right" : "sidebar.left",
      accessibilityDescription: nil
    )
    collapseButton.toolTip = collapsed ? "Expand sidebar" : "Collapse sidebar"
    collapseButton.state = collapsed ? .on : .off
    stack.spacing = collapsed ? 4 : 2
    homeButton.setCollapsed(collapsed)
    buttons.forEach { $0.setCollapsed(collapsed) }
    premiumButtons.forEach { $0.setCollapsed(collapsed) }
    if !collapsed {
      restoreDividerIfNeeded()
    }
  }

  func select(appId: String?) {
    let selectionChanged = selectedAppId != appId
    selectedAppId = appId
    homeButton.isSelected = appId == nil
    for (index, button) in buttons.enumerated() {
      button.isSelected = apps.indices.contains(index) && apps[index].id == appId
    }
    if selectionChanged {
      resetAppsScrollToTop()
    }
  }

  func selectApp(at index: Int) {
    guard apps.indices.contains(index) else { return }
    onSelect?(apps[index])
  }

  func splitView(
    _ splitView: NSSplitView, constrainMinCoordinate proposedMinimumPosition: CGFloat,
    ofSubviewAt dividerIndex: Int
  ) -> CGFloat {
    appsSectionCollapsed ? 36 : 150
  }

  func splitView(
    _ splitView: NSSplitView, constrainMaxCoordinate proposedMaximumPosition: CGFloat,
    ofSubviewAt dividerIndex: Int
  ) -> CGFloat {
    detailSectionCollapsed
      ? max(36, splitView.bounds.height - 36)
      : max(36, splitView.bounds.height - 118)
  }

  func splitViewDidResizeSubviews(_ notification: Notification) {
    guard !appsSectionCollapsed, !detailSectionCollapsed, detailPane.section != nil, !isCollapsed
    else { return }
    savedDividerPosition = appsPane.frame.height
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

    collapseButton.image = NSImage(systemSymbolName: "sidebar.left", accessibilityDescription: nil)
    collapseButton.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 16, weight: .medium)
    collapseButton.bezelStyle = .regularSquare
    collapseButton.setButtonType(.toggle)
    collapseButton.isBordered = false
    collapseButton.target = self
    collapseButton.action = #selector(toggleCollapse)
    collapseButton.toolTip = "Collapse sidebar"
    collapseButton.contentTintColor = .secondaryLabelColor
    collapseButton.wantsLayer = true
    collapseButton.layer?.cornerRadius = 9
    collapseButton.layer?.backgroundColor = NSColor.controlBackgroundColor.withAlphaComponent(0.5).cgColor
    collapseButton.translatesAutoresizingMaskIntoConstraints = false

    configureHeaderButton(
      appsHeader, title: "Apps", image: "chevron.down", action: #selector(toggleAppsSection))

    premiumCaption.font = .systemFont(ofSize: 11, weight: .medium)
    premiumCaption.textColor = .secondaryLabelColor
    premiumCaption.translatesAutoresizingMaskIntoConstraints = false

    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 2
    stack.translatesAutoresizingMaskIntoConstraints = false
    homeButton.target = self
    homeButton.action = #selector(goHome)
    homeButton.isSelected = true
    stack.addArrangedSubview(homeButton)

    appsScroll.drawsBackground = false
    appsScroll.hasVerticalScroller = true
    appsScroll.autohidesScrollers = true
    appsScroll.documentView = stack
    appsScroll.translatesAutoresizingMaskIntoConstraints = false
    appsPane.addSubview(appsHeader)
    appsPane.addSubview(appsScroll)
    NSLayoutConstraint.activate([
      appsHeader.leadingAnchor.constraint(equalTo: appsPane.leadingAnchor, constant: 8),
      appsHeader.trailingAnchor.constraint(equalTo: appsPane.trailingAnchor, constant: -8),
      appsHeader.topAnchor.constraint(equalTo: appsPane.topAnchor),
      appsHeader.heightAnchor.constraint(equalToConstant: 32),
      appsScroll.leadingAnchor.constraint(equalTo: appsPane.leadingAnchor),
      appsScroll.trailingAnchor.constraint(equalTo: appsPane.trailingAnchor),
      appsScroll.topAnchor.constraint(equalTo: appsHeader.bottomAnchor),
      appsScroll.bottomAnchor.constraint(equalTo: appsPane.bottomAnchor, constant: -4),
      stack.leadingAnchor.constraint(equalTo: appsScroll.contentView.leadingAnchor, constant: 8),
      stack.topAnchor.constraint(equalTo: appsScroll.contentView.topAnchor),
    ])

    detailPane.onSelect = { [weak self] id in self?.onSectionItemSelect?(id) }
    detailPane.onCreate = { [weak self] in self?.onSectionCreate?() }
    detailPane.onCollapsedChange = { [weak self] collapsed in
      guard let self else { return }
      self.detailSectionCollapsed = collapsed
      self.splitView.layoutSubtreeIfNeeded()
      if collapsed {
        let position = max(36, self.splitView.bounds.height - 36)
        self.splitView.setPosition(position, ofDividerAt: 0)
        self.splitView.layoutSubtreeIfNeeded()
        self.scrollAppsToTop()
      } else {
        self.restoreDividerIfNeeded()
      }
    }

    splitView.isVertical = false
    splitView.dividerStyle = .thin
    splitView.delegate = self
    splitView.translatesAutoresizingMaskIntoConstraints = false
    let initialSplitHeight = max(bounds.height - 88, 160)
    splitView.frame = NSRect(
      x: 0, y: 0, width: max(bounds.width, 1), height: initialSplitHeight)
    appsPane.frame = splitView.bounds
    detailPane.frame = NSRect(
      x: 0, y: initialSplitHeight, width: max(bounds.width, 1), height: 0)
    splitView.addSubview(appsPane)
    splitView.addSubview(detailPane)
    detailPane.setHostVisible(false)

    addSubview(brandIcon)
    addSubview(collapseButton)
    addSubview(title)
    addSubview(splitView)
    NSLayoutConstraint.activate([
      brandIcon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
      brandIcon.topAnchor.constraint(equalTo: topAnchor, constant: 42),
      brandIcon.widthAnchor.constraint(equalToConstant: 34),
      brandIcon.heightAnchor.constraint(equalToConstant: 34),
      collapseButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
      collapseButton.centerYAnchor.constraint(equalTo: brandIcon.centerYAnchor),
      collapseButton.widthAnchor.constraint(equalToConstant: 34),
      collapseButton.heightAnchor.constraint(equalToConstant: 34),
      title.leadingAnchor.constraint(equalTo: brandIcon.trailingAnchor, constant: 10),
      title.trailingAnchor.constraint(lessThanOrEqualTo: collapseButton.leadingAnchor, constant: -8),
      title.centerYAnchor.constraint(equalTo: brandIcon.centerYAnchor),
      splitView.leadingAnchor.constraint(equalTo: leadingAnchor),
      splitView.trailingAnchor.constraint(equalTo: trailingAnchor),
      splitView.topAnchor.constraint(equalTo: brandIcon.bottomAnchor, constant: 8),
      splitView.bottomAnchor.constraint(equalTo: bottomAnchor),
    ])
  }

  private func configureHeaderButton(
    _ button: NSButton, title: String, image: String, action: Selector
  ) {
    button.title = title
    button.image = NSImage(systemSymbolName: image, accessibilityDescription: nil)
    button.imagePosition = .imageLeading
    button.imageHugsTitle = true
    button.alignment = .left
    button.font = .systemFont(ofSize: 11, weight: .semibold)
    button.contentTintColor = .secondaryLabelColor
    button.bezelStyle = .regularSquare
    button.isBordered = false
    button.target = self
    button.action = action
    button.translatesAutoresizingMaskIntoConstraints = false
    button.setAccessibilityLabel("\(title) section")
  }

  private func restoreDividerIfNeeded() {
    guard detailPane.section != nil, !isCollapsed else { return }
    splitView.layoutSubtreeIfNeeded()
    if appsSectionCollapsed {
      splitView.setPosition(36, ofDividerAt: 0)
      detailPane.setHostVisible(true)
      return
    }
    if detailSectionCollapsed {
      splitView.setPosition(max(36, splitView.bounds.height - 36), ofDividerAt: 0)
      detailPane.setHostVisible(true)
      scrollAppsToTop()
      return
    }
    let maxPosition = max(150, splitView.bounds.height - 118)
    splitView.setPosition(min(savedDividerPosition, maxPosition), ofDividerAt: 0)
    detailPane.setHostVisible(true)
  }

  /// A resized/collapsed lower section must not preserve a stale bottom scroll
  /// offset in the now-taller Apps viewport.
  private func scrollAppsToTop() {
    appsPane.layoutSubtreeIfNeeded()
    appsScroll.layoutSubtreeIfNeeded()
    appsScroll.contentView.layoutSubtreeIfNeeded()
    stack.layoutSubtreeIfNeeded()
    guard let documentView = appsScroll.documentView else { return }
    let topY =
      documentView.isFlipped
      ? CGFloat(0)
      : max(0, documentView.bounds.height - appsScroll.contentView.bounds.height)
    appsScroll.contentView.scroll(to: NSPoint(x: 0, y: topY))
    appsScroll.reflectScrolledClipView(appsScroll.contentView)
  }

  /// Rendering and app selection can change the stack after its first layout
  /// pass. Reset now and once on the next main-loop turn so initial/app-switch
  /// navigation starts at Home without disturbing later deliberate scrolling.
  private func resetAppsScrollToTop() {
    scrollAppsToTop()
    DispatchQueue.main.async { [weak self] in
      self?.scrollAppsToTop()
    }
  }

  @objc private func toggleAppsSection(_ sender: NSButton) {
    appsSectionCollapsed.toggle()
    appsScroll.isHidden = appsSectionCollapsed
    appsHeader.image = NSImage(
      systemSymbolName: appsSectionCollapsed ? "chevron.right" : "chevron.down",
      accessibilityDescription: nil
    )
    appsHeader.setAccessibilityValue(appsSectionCollapsed ? "collapsed" : "expanded")
    if appsSectionCollapsed {
      splitView.setPosition(36, ofDividerAt: 0)
    } else {
      restoreDividerIfNeeded()
      scrollAppsToTop()
    }
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

final class AppProvidedSidebarView: NSView {
  var onSelect: ((String) -> Void)?
  var onCreate: (() -> Void)?
  var onCollapsedChange: ((Bool) -> Void)?
  private let headerButton = NSButton()
  private let createButton = NSButton()
  private let scrollView = NSScrollView()
  private let stack = NSStackView()
  private var rows: [AppSidebarItemButton] = []
  private var headerConstraints: [NSLayoutConstraint] = []
  private var expandedConstraints: [NSLayoutConstraint] = []
  private var collapsed = false
  fileprivate private(set) var section: AppSidebarSection?

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    configure()
  }

  required init?(coder: NSCoder) {
    super.init(coder: coder)
    configure()
  }

  func render(_ section: AppSidebarSection?) {
    self.section = section
    rows.forEach { $0.removeFromSuperview() }
    rows = []
    guard let section else {
      setHostVisible(false)
      return
    }
    headerButton.title = section.title
    headerButton.toolTip = "Collapse \(section.title)"
    createButton.isHidden = section.createLabel == nil
    createButton.toolTip = section.createLabel
    createButton.setAccessibilityLabel(section.createLabel ?? "Create item")
    for item in section.items {
      let row = AppSidebarItemButton(item: item)
      row.target = self
      row.action = #selector(selectItem(_:))
      row.isSelected = item.id == section.selectedItemId
      rows.append(row)
      stack.addArrangedSubview(row)
    }
  }

  func setHostVisible(_ visible: Bool) {
    if visible {
      NSLayoutConstraint.activate(headerConstraints)
      if !collapsed {
        NSLayoutConstraint.activate(expandedConstraints)
      }
    } else {
      NSLayoutConstraint.deactivate(headerConstraints)
      NSLayoutConstraint.deactivate(expandedConstraints)
    }
    headerButton.isHidden = !visible
    createButton.isHidden = !visible || section?.createLabel == nil
    scrollView.isHidden = !visible || collapsed
  }

  private func configure() {
    headerButton.image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)
    headerButton.imagePosition = .imageLeading
    headerButton.imageHugsTitle = true
    headerButton.alignment = .left
    headerButton.font = .systemFont(ofSize: 11, weight: .semibold)
    headerButton.contentTintColor = .secondaryLabelColor
    headerButton.bezelStyle = .regularSquare
    headerButton.isBordered = false
    headerButton.target = self
    headerButton.action = #selector(toggleSection)
    headerButton.translatesAutoresizingMaskIntoConstraints = false

    createButton.image = NSImage(systemSymbolName: "plus", accessibilityDescription: nil)
    createButton.bezelStyle = .regularSquare
    createButton.isBordered = false
    createButton.target = self
    createButton.action = #selector(createItem)
    createButton.translatesAutoresizingMaskIntoConstraints = false

    stack.orientation = .vertical
    stack.alignment = .leading
    stack.spacing = 2
    stack.translatesAutoresizingMaskIntoConstraints = false
    scrollView.drawsBackground = false
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.documentView = stack
    scrollView.translatesAutoresizingMaskIntoConstraints = false

    addSubview(headerButton)
    addSubview(createButton)
    addSubview(scrollView)
    headerConstraints = [
      headerButton.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
      headerButton.trailingAnchor.constraint(equalTo: createButton.leadingAnchor, constant: -2),
      headerButton.topAnchor.constraint(equalTo: topAnchor),
      headerButton.heightAnchor.constraint(equalToConstant: 32),
      createButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
      createButton.centerYAnchor.constraint(equalTo: headerButton.centerYAnchor),
      createButton.widthAnchor.constraint(equalToConstant: 28),
      createButton.heightAnchor.constraint(equalToConstant: 28),
    ]
    expandedConstraints = [
      scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
      scrollView.trailingAnchor.constraint(equalTo: trailingAnchor),
      scrollView.topAnchor.constraint(equalTo: headerButton.bottomAnchor),
      scrollView.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -4),
      stack.leadingAnchor.constraint(equalTo: scrollView.contentView.leadingAnchor, constant: 8),
      stack.topAnchor.constraint(equalTo: scrollView.contentView.topAnchor),
    ]
    setHostVisible(false)
  }

  @objc private func toggleSection() {
    collapsed.toggle()
    if collapsed {
      NSLayoutConstraint.deactivate(expandedConstraints)
      scrollView.isHidden = true
    }
    headerButton.image = NSImage(
      systemSymbolName: collapsed ? "chevron.right" : "chevron.down",
      accessibilityDescription: nil
    )
    headerButton.toolTip = "\(collapsed ? "Expand" : "Collapse") \(section?.title ?? "section")"
    headerButton.setAccessibilityValue(collapsed ? "collapsed" : "expanded")
    onCollapsedChange?(collapsed)
    if !collapsed {
      NSLayoutConstraint.activate(expandedConstraints)
      scrollView.isHidden = false
    }
  }

  @objc private func selectItem(_ sender: AppSidebarItemButton) {
    onSelect?(sender.itemId)
  }

  @objc private func createItem() {
    onCreate?()
  }
}

final class AppSidebarItemButton: NSButton {
  let itemId: String
  private let fullTitle: String
  private let subtitle: String?
  var isSelected = false {
    didSet {
      needsDisplay = true
      contentTintColor = isSelected ? .labelColor : .secondaryLabelColor
    }
  }

  init(item: AppSidebarSectionItem) {
    itemId = item.id
    fullTitle = item.title
    subtitle = item.subtitle
    super.init(frame: .zero)
    title = item.title
    toolTip = item.subtitle ?? item.title
    image = item.systemImage.flatMap {
      NSImage(systemSymbolName: $0, accessibilityDescription: nil)
    }
    configure()
  }

  required init?(coder: NSCoder) {
    itemId = ""
    fullTitle = ""
    subtitle = nil
    super.init(coder: coder)
    configure()
  }

  override func draw(_ dirtyRect: NSRect) {
    if isSelected {
      NSColor.labelColor.withAlphaComponent(0.1).setFill()
      NSBezierPath(roundedRect: bounds.insetBy(dx: 0, dy: 1), xRadius: 10, yRadius: 10).fill()
    }
    super.draw(dirtyRect)
  }

  private func configure() {
    bezelStyle = .regularSquare
    isBordered = false
    alignment = .left
    font = .systemFont(ofSize: 13, weight: .medium)
    imagePosition = image == nil ? .noImage : .imageLeading
    imageHugsTitle = image != nil
    lineBreakMode = .byTruncatingTail
    contentTintColor = .secondaryLabelColor
    translatesAutoresizingMaskIntoConstraints = false
    widthAnchor.constraint(equalToConstant: 216).isActive = true
    heightAnchor.constraint(equalToConstant: 34).isActive = true
    setAccessibilityLabel(fullTitle)
    if let subtitle { setAccessibilityHelp(subtitle) }
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
    NSSize(width: NSView.noIntrinsicMetric, height: collapsed ? 44 : 34)
  }

  func setCollapsed(_ collapsed: Bool) {
    self.collapsed = collapsed
    title = collapsed ? "" : fullTitle
    alignment = collapsed ? .center : .left
    imageHugsTitle = !collapsed
    widthConstraint?.constant = collapsed ? 52 : 216
    heightConstraint?.constant = collapsed ? 44 : 34
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
    widthConstraint = widthAnchor.constraint(equalToConstant: 216)
    heightConstraint = heightAnchor.constraint(equalToConstant: 34)
    widthConstraint?.isActive = true
    heightConstraint?.isActive = true
  }
}
