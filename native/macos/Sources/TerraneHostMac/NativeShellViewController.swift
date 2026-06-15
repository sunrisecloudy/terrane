import AppKit

struct MacAppCatalogItem: Equatable {
    let id: String
    let name: String
    let version: String
    let description: String
    let contentRatingLabel: String?
}

final class MacAppCatalog {
    func loadBundledApps() throws -> [MacAppCatalogItem] {
        guard let examplesDirectory = RuntimeResourceLocator.examplesDirectoryURL() else {
            return []
        }
        let entries = try FileManager.default.contentsOfDirectory(
            at: examplesDirectory,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        )

        var apps: [MacAppCatalogItem] = []
        for appDirectory in entries {
            guard (try? appDirectory.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true else {
                continue
            }
            do {
                apps.append(try Self.readManifest(at: appDirectory.appendingPathComponent("manifest.json")))
            } catch {
                fputs("Terrane skipped bundled app manifest \(appDirectory.lastPathComponent): \(error)\n", stderr)
            }
        }

        return apps.sorted { left, right in
            left.name.localizedStandardCompare(right.name) == .orderedAscending
        }
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
            contentRatingLabel: rating?["label"] as? String
        )
    }
}

final class NativeShellViewController: NSSplitViewController {
    private let sidebarController = NativeSidebarViewController()
    private let workspaceController = NativeWorkspaceViewController()
    private let catalog: MacAppCatalog
    private var sidebarItem: NSSplitViewItem?

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
}

final class NativeSidebarViewController: NSViewController, NSTableViewDataSource, NSTableViewDelegate {
    var onSelectApp: ((MacAppCatalogItem) -> Void)?

    private var apps: [MacAppCatalogItem] = []
    private let tableView = NSTableView()
    private let scrollView = NSScrollView()
    private let titleField = NSTextField(labelWithString: "Apps")

    override func loadView() {
        let root = NSVisualEffectView()
        root.material = .sidebar
        root.blendingMode = .behindWindow
        root.state = .active
        root.isEmphasized = false

        titleField.font = .systemFont(ofSize: 11, weight: .semibold)
        titleField.textColor = .secondaryLabelColor
        titleField.stringValue = titleField.stringValue.uppercased()
        titleField.maximumNumberOfLines = 1

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("apps"))
        column.title = "Apps"
        tableView.addTableColumn(column)
        tableView.headerView = nil
        tableView.rowHeight = 28
        tableView.intercellSpacing = NSSize(width: 0, height: 0)
        tableView.style = .sourceList
        tableView.dataSource = self
        tableView.delegate = self
        tableView.backgroundColor = .clear
        tableView.usesAlternatingRowBackgroundColors = false
        tableView.allowsEmptySelection = false

        scrollView.documentView = tableView
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        root.addSubview(titleField)
        root.addSubview(scrollView)

        view = root
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        let horizontalInset: CGFloat = 10
        let topInset: CGFloat = 20
        let sectionHeight: CGFloat = 18
        titleField.frame = NSRect(
            x: horizontalInset + 7,
            y: max(0, view.bounds.height - topInset - sectionHeight),
            width: max(0, view.bounds.width - horizontalInset * 2 - 14),
            height: sectionHeight
        )
        scrollView.frame = NSRect(
            x: horizontalInset,
            y: 12,
            width: max(0, view.bounds.width - horizontalInset * 2),
            height: max(0, view.bounds.height - topInset - sectionHeight - 18)
        )
    }

    func updateApps(_ apps: [MacAppCatalogItem]) {
        self.apps = apps
        tableView.reloadData()
    }

    func select(appId: String) {
        guard let index = apps.firstIndex(where: { $0.id == appId }) else { return }
        tableView.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        tableView.scrollRowToVisible(index)
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        apps.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let identifier = NSUserInterfaceItemIdentifier("app-cell")
        let cell = tableView.makeView(withIdentifier: identifier, owner: self) as? AppSidebarCellView
            ?? AppSidebarCellView(identifier: identifier)
        cell.configure(with: apps[row])
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = tableView.selectedRow
        guard row >= 0, row < apps.count else { return }
        onSelectApp?(apps[row])
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
    private let headerView = NSView()
    private let titleField = NSTextField(labelWithString: "No app selected")
    private let descriptionField = NSTextField(labelWithString: "Choose an app from the sidebar.")
    private let reloadButton = NSButton(title: "Reload", target: nil, action: nil)
    private let webHostView = WebHostView(nativeHostModeEnabled: true)
    private var selectedApp: MacAppCatalogItem?

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
        selectedApp = app
        titleField.stringValue = app.name
        descriptionField.stringValue = app.description
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = true
        webHostView.mountApp(id: app.id)
    }

    func showEmptyState() {
        selectedApp = nil
        titleField.stringValue = "No bundled apps found"
        descriptionField.stringValue = "Terrane could not find bundled generated apps."
        descriptionField.textColor = .secondaryLabelColor
        reloadButton.isEnabled = false
    }

    func showCatalogError(_ error: Error) {
        selectedApp = nil
        titleField.stringValue = "Could not load bundled apps"
        descriptionField.stringValue = error.localizedDescription
        descriptionField.textColor = .systemRed
        reloadButton.isEnabled = false
    }

    private func showRuntimeError(_ message: String) {
        descriptionField.stringValue = message
        descriptionField.textColor = .systemRed
        reloadButton.isEnabled = selectedApp != nil
    }

    @objc private func reloadSelectedApp() {
        guard let selectedApp else { return }
        descriptionField.stringValue = selectedApp.description
        descriptionField.textColor = .secondaryLabelColor
        webHostView.mountApp(id: selectedApp.id)
    }
}
