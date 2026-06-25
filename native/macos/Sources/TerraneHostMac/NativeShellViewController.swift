import AppKit

struct MacAppCatalogItem: Equatable {
    let id: String
    let name: String
    let version: String
    let description: String
    let contentRatingLabel: String?
    let isPremium: Bool
}

final class MacAppCatalog {
    func loadBundledApps() throws -> [MacAppCatalogItem] {
        var apps: [MacAppCatalogItem] = []
        var seenIds = Set<String>()

        // Bundled (free) apps are the primary catalog and are always flat (one directory
        // per app); a read failure here is surfaced to the shell as a catalog error.
        if let examplesDirectory = RuntimeResourceLocator.examplesDirectoryURL() {
            for item in try Self.loadApps(in: examplesDirectory, maxDepth: 1) where seenIds.insert(item.id).inserted {
                apps.append(item)
            }
        }

        // Premium apps are an optional dev/host seam and may be nested under product
        // folders (e.g. hold-dear/admin), so scan a couple of levels deep. Absence or read
        // errors are non-fatal so the public host stays usable without a Premium checkout.
        if let premiumDirectory = RuntimeResourceLocator.premiumAppsDirectoryURL() {
            let premiumApps = (try? Self.loadApps(in: premiumDirectory, maxDepth: 2)) ?? []
            for item in premiumApps where seenIds.insert(item.id).inserted {
                apps.append(item)
            }
        }

        return apps.sorted { left, right in
            left.name.localizedStandardCompare(right.name) == .orderedAscending
        }
    }

    private static func loadApps(in directory: URL, maxDepth: Int) throws -> [MacAppCatalogItem] {
        let topLevel = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )

        var appDirectories: [URL] = []
        for child in topLevel where isDirectory(child) && child.lastPathComponent != "node_modules" {
            collectAppDirectories(in: child, depth: 1, maxDepth: maxDepth, into: &appDirectories)
        }

        var apps: [MacAppCatalogItem] = []
        for appDirectory in appDirectories.sorted(by: { $0.path < $1.path }) {
            let manifestURL = appDirectory.appendingPathComponent("manifest.json")
            do {
                let item = try readManifest(at: manifestURL)
                // Record where this app id actually lives so the resource layer can serve
                // it even when the id differs from the directory path.
                AppDirectoryRegistry.shared.register(appId: item.id, directory: appDirectory)
                apps.append(item)
            } catch {
                fputs("Terrane skipped app manifest \(appDirectory.lastPathComponent): \(error)\n", stderr)
            }
        }
        return apps
    }

    /// Collects directories that directly contain a `manifest.json`, descending up to
    /// `maxDepth` levels. A directory that is itself an app is not descended into.
    private static func collectAppDirectories(in directory: URL, depth: Int, maxDepth: Int, into result: inout [URL]) {
        if FileManager.default.fileExists(atPath: directory.appendingPathComponent("manifest.json").path) {
            result.append(directory)
            return
        }
        guard depth < maxDepth else { return }
        guard let children = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else {
            return
        }
        for child in children where isDirectory(child) && child.lastPathComponent != "node_modules" {
            collectAppDirectories(in: child, depth: depth + 1, maxDepth: maxDepth, into: &result)
        }
    }

    private static func isDirectory(_ url: URL) -> Bool {
        (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
    }

    private static func readManifest(at manifestURL: URL) throws -> MacAppCatalogItem {
        let data = try Data(contentsOf: manifestURL)
        guard let manifest = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = manifest["id"] as? String,
              let name = manifest["name"] as? String
        else {
            throw CocoaError(.fileReadCorruptFile)
        }

        let rating = manifest["contentRating"] as? [String: Any]
        return MacAppCatalogItem(
            id: id,
            name: name,
            version: manifest["version"] as? String ?? "",
            description: manifest["description"] as? String ?? "",
            contentRatingLabel: rating?["label"] as? String,
            isPremium: manifest["premium"] as? Bool ?? false
        )
    }
}

final class NativeShellViewController: NSSplitViewController {
    private let sidebarController = NativeSidebarViewController()
    private let workspaceController = NativeWorkspaceViewController()
    private let catalog: MacAppCatalog
    private var sidebarItem: NSSplitViewItem?
    var onEngineRoomVisibilityChanged: ((Bool) -> Void)?

    init(catalog: MacAppCatalog = MacAppCatalog()) {
        self.catalog = catalog
        super.init(nibName: nil, bundle: nil)
        preferredContentSize = NativeWindowConfiguration.preferredContentSize

        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebarController)
        sidebarItem.minimumThickness = 200
        sidebarItem.maximumThickness = 300
        sidebarItem.preferredThicknessFraction = 0.23
        sidebarItem.canCollapse = true
        self.sidebarItem = sidebarItem

        let workspaceItem = NSSplitViewItem(viewController: workspaceController)
        workspaceItem.minimumThickness = 560

        addSplitViewItem(sidebarItem)
        addSplitViewItem(workspaceItem)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        splitView.dividerStyle = .thin

        let apps: [MacAppCatalogItem]
        do {
            apps = try catalog.loadBundledApps()
        } catch {
            sidebarController.updateApps([])
            workspaceController.showCatalogError(error)
            fputs("Terrane could not load bundled app catalog: \(error)\n", stderr)
            return
        }
        sidebarController.updateApps(apps)
        sidebarController.setEngineRoomVisible(NativeShellPreferences.isEngineRoomVisible)
        workspaceController.updateApps(apps)
        workspaceController.onRuntimeSelectedApp = { [weak sidebarController] appId in
            sidebarController?.select(appId: appId)
        }
        sidebarController.onSelectMarketplace = { [weak self] in
            self?.workspaceController.showMarketplace()
        }
        sidebarController.onSelectEngineRoom = { [weak self] in
            self?.workspaceController.showEngineRoom()
        }
        sidebarController.onSelectApp = { [weak self] app in
            self?.workspaceController.select(app)
        }

        if let firstApp = apps.first(where: { $0.id == "notes-lite" }) ?? apps.first {
            sidebarController.select(appId: firstApp.id)
            workspaceController.select(firstApp)
        } else {
            workspaceController.showEmptyState()
        }
    }

    func toggleSidebar() {
        guard let sidebarItem else { return }
        sidebarItem.animator().isCollapsed.toggle()
    }

    func setEngineRoomVisible(_ visible: Bool) {
        NativeShellPreferences.setEngineRoomVisible(visible)
        sidebarController.setEngineRoomVisible(visible)
        workspaceController.setEngineRoomVisible(visible)
        onEngineRoomVisibilityChanged?(visible)
    }
}

final class NativeSidebarViewController: NSViewController, NSTableViewDataSource, NSTableViewDelegate {
    private enum SidebarRow: Equatable {
        case header(String)
        case app(MacAppCatalogItem)
    }

    var onSelectMarketplace: (() -> Void)?
    var onSelectEngineRoom: (() -> Void)?
    var onSelectApp: ((MacAppCatalogItem) -> Void)?

    private var rows: [SidebarRow] = []
    private var isEngineRoomVisible = false
    private var suppressSelectionCallback = false
    private let tableView = NSTableView()
    private let scrollView = NSScrollView()
    private let marketplaceTitleField = NSTextField(labelWithString: "Marketplace")
    private let marketplaceButton = SidebarActionButton(title: "Marketplace", symbolName: "storefront")
    private let engineRoomTitleField = NSTextField(labelWithString: "Inspect")
    private let engineRoomButton = SidebarActionButton(title: "Engine Room", symbolName: "wrench.and.screwdriver")

    override func loadView() {
        let root = NSVisualEffectView()
        root.material = .sidebar
        root.blendingMode = .behindWindow
        root.state = .active
        root.isEmphasized = false

        marketplaceTitleField.font = .systemFont(ofSize: 11, weight: .semibold)
        marketplaceTitleField.textColor = .secondaryLabelColor
        marketplaceTitleField.stringValue = marketplaceTitleField.stringValue.uppercased()
        marketplaceTitleField.maximumNumberOfLines = 1

        marketplaceButton.target = self
        marketplaceButton.action = #selector(selectMarketplace)

        engineRoomTitleField.font = .systemFont(ofSize: 11, weight: .semibold)
        engineRoomTitleField.textColor = .secondaryLabelColor
        engineRoomTitleField.stringValue = engineRoomTitleField.stringValue.uppercased()
        engineRoomTitleField.maximumNumberOfLines = 1

        engineRoomButton.target = self
        engineRoomButton.action = #selector(selectEngineRoom)

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("apps"))
        column.title = "Apps"
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.rowHeight = 28
        tableView.intercellSpacing = NSSize(width: 0, height: 0)
        tableView.style = .sourceList
        tableView.floatsGroupRows = false
        tableView.dataSource = self
        tableView.delegate = self
        tableView.backgroundColor = .clear
        tableView.usesAlternatingRowBackgroundColors = false
        tableView.allowsEmptySelection = false

        scrollView.documentView = tableView
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        root.addSubview(marketplaceTitleField)
        root.addSubview(marketplaceButton)
        root.addSubview(engineRoomTitleField)
        root.addSubview(engineRoomButton)
        root.addSubview(scrollView)

        view = root
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        let horizontalInset: CGFloat = 10
        let topInset: CGFloat = 20
        let sectionHeight: CGFloat = 18
        let rowHeight: CGFloat = 28
        let sectionGap: CGFloat = 14

        marketplaceTitleField.frame = NSRect(
            x: horizontalInset + 7,
            y: max(0, view.bounds.height - topInset - sectionHeight),
            width: max(0, view.bounds.width - horizontalInset * 2 - 14),
            height: sectionHeight
        )
        marketplaceButton.frame = NSRect(
            x: horizontalInset,
            y: max(0, marketplaceTitleField.frame.minY - rowHeight - 6),
            width: max(0, view.bounds.width - horizontalInset * 2),
            height: rowHeight
        )
        engineRoomTitleField.isHidden = !isEngineRoomVisible
        engineRoomButton.isHidden = !isEngineRoomVisible
        let lastActionButtonMinY: CGFloat
        if isEngineRoomVisible {
            engineRoomTitleField.frame = NSRect(
                x: horizontalInset + 7,
                y: max(0, marketplaceButton.frame.minY - sectionGap - sectionHeight),
                width: max(0, view.bounds.width - horizontalInset * 2 - 14),
                height: sectionHeight
            )
            engineRoomButton.frame = NSRect(
                x: horizontalInset,
                y: max(0, engineRoomTitleField.frame.minY - rowHeight - 6),
                width: max(0, view.bounds.width - horizontalInset * 2),
                height: rowHeight
            )
            lastActionButtonMinY = engineRoomButton.frame.minY
        } else {
            lastActionButtonMinY = marketplaceButton.frame.minY
        }
        // The "Apps" / "Premium Apps" section headers now live inside the table as
        // group rows, so the list starts directly below the action buttons.
        let scrollTop = max(0, lastActionButtonMinY - sectionGap)
        scrollView.frame = NSRect(
            x: horizontalInset,
            y: 12,
            width: max(0, view.bounds.width - horizontalInset * 2),
            height: max(0, scrollTop - 12)
        )
    }

    func updateApps(_ apps: [MacAppCatalogItem]) {
        let freeApps = apps.filter { !$0.isPremium }
        let premiumApps = apps.filter { $0.isPremium }
        var rows: [SidebarRow] = []
        if !freeApps.isEmpty {
            rows.append(.header("Apps"))
            rows.append(contentsOf: freeApps.map(SidebarRow.app))
        }
        if !premiumApps.isEmpty {
            rows.append(.header("Premium Apps"))
            rows.append(contentsOf: premiumApps.map(SidebarRow.app))
        }
        self.rows = rows
        tableView.reloadData()
    }

    func setEngineRoomVisible(_ visible: Bool) {
        isEngineRoomVisible = visible
        if !visible, engineRoomButton.isSelectedForSidebar {
            engineRoomButton.isSelectedForSidebar = false
        }
        view.needsLayout = true
    }

    func select(appId: String) {
        guard let index = rows.firstIndex(where: { row in
            if case .app(let app) = row { return app.id == appId }
            return false
        }) else { return }
        marketplaceButton.isSelectedForSidebar = false
        engineRoomButton.isSelectedForSidebar = false
        suppressSelectionCallback = true
        tableView.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        suppressSelectionCallback = false
        tableView.scrollRowToVisible(index)
    }

    func selectMarketplaceRow() {
        tableView.deselectAll(nil)
        marketplaceButton.isSelectedForSidebar = true
        engineRoomButton.isSelectedForSidebar = false
    }

    func selectEngineRoomRow() {
        guard isEngineRoomVisible else { return }
        tableView.deselectAll(nil)
        marketplaceButton.isSelectedForSidebar = false
        engineRoomButton.isSelectedForSidebar = true
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        switch rows[row] {
        case .header(let title):
            let identifier = NSUserInterfaceItemIdentifier("section-header")
            let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? SidebarSectionHeaderView
                ?? SidebarSectionHeaderView(identifier: identifier)
            cell.configure(title: title)
            return cell
        case .app(let app):
            let identifier = NSUserInterfaceItemIdentifier("app-cell")
            let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? AppSidebarCellView
                ?? AppSidebarCellView(identifier: identifier)
            cell.configure(with: app)
            return cell
        }
    }

    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
        if case .header = rows[row] { return true }
        return false
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        if case .app = rows[row] { return true }
        return false
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        if case .header = rows[row] { return 24 }
        return 28
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard !suppressSelectionCallback else { return }
        let row = tableView.selectedRow
        guard row >= 0, row < rows.count, case .app(let app) = rows[row] else { return }
        marketplaceButton.isSelectedForSidebar = false
        engineRoomButton.isSelectedForSidebar = false
        onSelectApp?(app)
    }

    @objc private func selectMarketplace() {
        selectMarketplaceRow()
        onSelectMarketplace?()
    }

    @objc private func selectEngineRoom() {
        selectEngineRoomRow()
        onSelectEngineRoom?()
    }
}

final class SidebarActionButton: NSButton {
    var isSelectedForSidebar = false {
        didSet {
            needsDisplay = true
            titleLabel.textColor = isSelectedForSidebar ? .alternateSelectedControlTextColor : .labelColor
            iconView.contentTintColor = isSelectedForSidebar ? .alternateSelectedControlTextColor : .secondaryLabelColor
        }
    }

    private let iconView = NSImageView()
    private let titleLabel = NSTextField(labelWithString: "")

    init(title: String, symbolName: String) {
        super.init(frame: .zero)
        isBordered = false
        self.title = ""
        attributedTitle = NSAttributedString(string: "")
        attributedAlternateTitle = NSAttributedString(string: "")
        imagePosition = .noImage
        focusRingType = .none
        setButtonType(.momentaryChange)
        setAccessibilityLabel(title)

        iconView.image = NSImage(systemSymbolName: symbolName, accessibilityDescription: nil)
        iconView.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 15, weight: .regular)
        iconView.contentTintColor = .secondaryLabelColor

        titleLabel.stringValue = title
        titleLabel.font = .systemFont(ofSize: 13)
        titleLabel.lineBreakMode = .byTruncatingTail
        titleLabel.maximumNumberOfLines = 1

        addSubview(iconView)
        addSubview(titleLabel)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func draw(_ dirtyRect: NSRect) {
        if isSelectedForSidebar {
            NSColor.controlAccentColor.setFill()
            NSBezierPath(roundedRect: bounds, xRadius: 7, yRadius: 7).fill()
        }
    }

    override func layout() {
        super.layout()
        let iconSize: CGFloat = 18
        let inset: CGFloat = 7
        iconView.frame = NSRect(
            x: inset,
            y: floor((bounds.height - iconSize) / 2),
            width: iconSize,
            height: iconSize
        )
        titleLabel.frame = NSRect(
            x: iconView.frame.maxX + 7,
            y: floor((bounds.height - 17) / 2),
            width: max(0, bounds.width - iconView.frame.maxX - 14),
            height: 17
        )
    }
}

final class SidebarSectionHeaderView: NSTableCellView {
    private let titleLabel = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        titleLabel.font = .systemFont(ofSize: 11, weight: .semibold)
        titleLabel.textColor = .secondaryLabelColor
        titleLabel.lineBreakMode = .byTruncatingTail
        titleLabel.maximumNumberOfLines = 1
        addSubview(titleLabel)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func layout() {
        super.layout()
        // Match the inset of the Marketplace / Inspect labels above the list.
        titleLabel.frame = NSRect(
            x: 7,
            y: max(0, bounds.height - 16),
            width: max(0, bounds.width - 14),
            height: 14
        )
    }

    func configure(title: String) {
        titleLabel.stringValue = title.uppercased()
    }
}

final class AppSidebarCellView: NSTableCellView {
    private let iconView = NSImageView()
    private let titleField = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        iconView.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 15, weight: .regular)
        iconView.contentTintColor = .secondaryLabelColor

        titleField.font = .systemFont(ofSize: 13)
        titleField.lineBreakMode = .byTruncatingTail
        titleField.maximumNumberOfLines = 1

        addSubview(iconView)
        addSubview(titleField)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func layout() {
        super.layout()
        let iconSize: CGFloat = 18
        let inset: CGFloat = 7
        iconView.frame = NSRect(
            x: inset,
            y: floor((bounds.height - iconSize) / 2),
            width: iconSize,
            height: iconSize
        )
        titleField.frame = NSRect(
            x: iconView.frame.maxX + 7,
            y: floor((bounds.height - 17) / 2),
            width: max(0, bounds.width - iconView.frame.maxX - 14),
            height: 17
        )
    }

    func configure(with app: MacAppCatalogItem) {
        iconView.image = NSImage(systemSymbolName: "app.dashed", accessibilityDescription: nil)
        titleField.stringValue = app.name
    }
}

final class NativeWorkspaceViewController: NSViewController {
    private enum Selection {
        case app(MacAppCatalogItem)
        case marketplace
        case engineRoom
    }

    private let headerView = NSView()
    private let titleField = NSTextField(labelWithString: "No app selected")
    private let descriptionField = NSTextField(labelWithString: "Choose an app from the sidebar.")
    private let reloadButton = NSButton(title: "Reload", target: nil, action: nil)
    private let webHostView = WebHostView(nativeHostModeEnabled: true)
    private var selection: Selection?
    private var appsById: [String: MacAppCatalogItem] = [:]
    var onRuntimeSelectedApp: ((String) -> Void)?

    override func loadView() {
        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor

        headerView.wantsLayer = true
        headerView.layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
        headerView.layer?.borderColor = NSColor.separatorColor.cgColor
        headerView.layer?.borderWidth = 1

        titleField.font = .systemFont(ofSize: 17, weight: .semibold)
        titleField.lineBreakMode = .byTruncatingTail
        titleField.maximumNumberOfLines = 1

        descriptionField.font = .systemFont(ofSize: 12)
        descriptionField.textColor = .secondaryLabelColor
        descriptionField.lineBreakMode = .byTruncatingTail
        descriptionField.maximumNumberOfLines = 1

        reloadButton.bezelStyle = .rounded
        reloadButton.target = self
        reloadButton.action = #selector(reloadSelectedApp)
        reloadButton.isEnabled = false
        webHostView.onNativeRuntimeError = { [weak self] message in
            self?.showRuntimeError(message)
        }
        webHostView.onNativeAppMounted = { [weak self] appId in
            self?.showRuntimeMountedApp(appId: appId)
        }

        headerView.addSubview(titleField)
        headerView.addSubview(descriptionField)
        headerView.addSubview(reloadButton)
        root.addSubview(headerView)
        root.addSubview(webHostView)

        view = root
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        let headerHeight: CGFloat = 68
        headerView.frame = NSRect(x: 0, y: max(0, view.bounds.height - headerHeight), width: view.bounds.width, height: headerHeight)
        webHostView.frame = NSRect(x: 0, y: 0, width: view.bounds.width, height: max(0, view.bounds.height - headerHeight))

        reloadButton.sizeToFit()
        let reloadSize = reloadButton.frame.size
        reloadButton.frame = NSRect(
            x: max(16, headerView.bounds.width - reloadSize.width - 18),
            y: 20,
            width: reloadSize.width,
            height: reloadSize.height
        )
        let textWidth = max(0, reloadButton.frame.minX - 32)
        titleField.frame = NSRect(x: 18, y: 36, width: textWidth, height: 22)
        descriptionField.frame = NSRect(x: 18, y: 16, width: textWidth, height: 18)
    }

    func select(_ app: MacAppCatalogItem) {
        selection = .app(app)
        titleField.stringValue = app.name
        descriptionField.stringValue = app.description
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = true
        webHostView.mountApp(id: app.id)
    }

    func updateApps(_ apps: [MacAppCatalogItem]) {
        appsById = Dictionary(uniqueKeysWithValues: apps.map { ($0.id, $0) })
    }

    func showMarketplace() {
        selection = .marketplace
        titleField.stringValue = "Marketplace"
        descriptionField.stringValue = "Browse Terrane Premium apps from the Premium server."
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = true
        webHostView.showMarketplace()
    }

    func showEngineRoom() {
        guard NativeShellPreferences.isEngineRoomVisible else { return }
        selection = .engineRoom
        titleField.stringValue = "Engine Room"
        descriptionField.stringValue = "Inspect raw app and platform debug data."
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = true
        webHostView.showEngineRoom()
    }

    func setEngineRoomVisible(_ visible: Bool) {
        if !visible, case .engineRoom = selection {
            selection = nil
            reloadButton.isEnabled = false
        }
    }

    func showEmptyState() {
        selection = nil
        titleField.stringValue = "No bundled apps found"
        descriptionField.stringValue = "Terrane could not find bundled generated apps."
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = false
    }

    func showCatalogError(_ error: Error) {
        selection = nil
        titleField.stringValue = "Could not load bundled apps"
        descriptionField.stringValue = error.localizedDescription
        descriptionField.textColor = .systemRed
        reloadButton.isEnabled = false
    }

    private func showRuntimeError(_ message: String) {
        descriptionField.stringValue = message
        descriptionField.textColor = .systemRed
        reloadButton.isEnabled = selection != nil
    }

    private func showRuntimeMountedApp(appId: String) {
        guard let app = appsById[appId] else { return }
        selection = .app(app)
        titleField.stringValue = app.name
        descriptionField.stringValue = app.description
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = true
        onRuntimeSelectedApp?(app.id)
    }

    @objc private func reloadSelectedApp() {
        switch selection {
        case .app(let selectedApp):
            descriptionField.stringValue = selectedApp.description
            descriptionField.textColor = .secondaryLabelColor
            webHostView.mountApp(id: selectedApp.id)
        case .marketplace:
            descriptionField.stringValue = "Browse Terrane Premium apps from the Premium server."
            descriptionField.textColor = .secondaryLabelColor
            webHostView.showMarketplace()
        case .engineRoom:
            descriptionField.stringValue = "Inspect raw app and platform debug data."
            descriptionField.textColor = .secondaryLabelColor
            webHostView.showEngineRoom()
        case nil:
            return
        }
    }
}
