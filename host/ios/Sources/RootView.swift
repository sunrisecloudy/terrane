import SwiftUI

struct RootView: View {
  @ObservedObject var model: TerraneIOSModel

  var body: some View {
    TabView {
      NavigationStack {
        AppListView(model: model)
      }
      .tabItem {
        Label("Apps", systemImage: "square.grid.2x2")
      }

      ForEach(model.orderedApps.filter { model.isPinned($0.id) }) { app in
        NavigationStack {
          AppHostView(app: app, model: model)
        }
        .tabItem {
          Label(app.name, systemImage: NativeAppIconCatalog.systemName(for: app))
        }
        .accessibilityIdentifier("pinned-tab.\(app.id)")
      }

      NavigationStack {
        PremiumAccountView(
          controller: model.premiumAccount,
          configuration: model.configuration,
          healthAutoSync: model.healthAutoSync
        )
      }
      .tabItem {
        Label("Account", systemImage: "person.crop.circle")
      }
    }
    .alert(item: $model.permissionPrompt) { prompt in
      Alert(
        title: Text("Allow \(prompt.appName) to access resources?"),
        message: Text("The app is requesting: \(prompt.resources.joined(separator: ", "))."),
        primaryButton: .default(Text("Allow")) {
          model.resolvePermission(approved: true)
        },
        secondaryButton: .cancel {
          model.resolvePermission(approved: false)
        }
      )
    }
  }
}

private struct AppListView: View {
  @ObservedObject var model: TerraneIOSModel

  var body: some View {
    Group {
      if model.apps.isEmpty {
        ContentUnavailableView(
          "No local apps",
          systemImage: "square.grid.2x2",
          description: Text("Terrane could not find its bundled local apps.")
        )
      } else {
        List(model.orderedApps) { app in
          NavigationLink(value: app.id) {
            Label {
              HStack {
                VStack(alignment: .leading) {
                  Text(app.name)
                  Text(app.id)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                Spacer()
                if model.isPinned(app.id) {
                  Image(systemName: "pin.fill")
                    .foregroundStyle(.tint)
                    .accessibilityLabel("Pinned")
                    .accessibilityIdentifier("app.\(app.id).pinned")
                }
              }
            } icon: {
              Image(systemName: NativeAppIconCatalog.systemName(for: app))
                .frame(width: 28)
                .accessibilityLabel("\(app.name) icon")
                .accessibilityIdentifier("app.\(app.id).icon")
            }
          }
          .accessibilityIdentifier("app.\(app.id)")
          .swipeActions(edge: .leading, allowsFullSwipe: true) {
            pinButton(for: app)
          }
          .contextMenu {
            pinButton(for: app)
          }
        }
      }
    }
    .navigationTitle("Terrane")
    .navigationDestination(for: String.self) { id in
      if let app = model.apps.first(where: { $0.id == id }) {
        AppHostView(app: app, model: model)
      }
    }
    .safeAreaInset(edge: .bottom) {
      runtimeStatus
    }
  }

  @ViewBuilder
  private var runtimeStatus: some View {
    VStack(spacing: 0) {
      if !model.healthSyncStatus.isEmpty {
        Text(model.healthSyncStatus)
          .accessibilityIdentifier("health.sync.status")
          .font(.caption)
          .foregroundStyle(
            model.healthSyncStatus.contains("failed") ? Color.red : Color.secondary
          )
          .padding(.vertical, 6)
          .frame(maxWidth: .infinity)
          .background(.bar)
      }
      if !model.startupError.isEmpty {
        Text(model.startupError)
          .font(.caption)
          .foregroundStyle(.red)
          .padding(.vertical, 6)
          .frame(maxWidth: .infinity)
          .background(.bar)
      }
      if case .unavailable(let message) = model.runtime.availability {
        Text(message)
          .font(.caption)
          .foregroundStyle(.red)
          .padding(.vertical, 6)
          .frame(maxWidth: .infinity)
          .background(.bar)
      }
    }
  }

  private func pinButton(for app: TerraneApp) -> some View {
    let isPinned = model.isPinned(app.id)
    return Button {
      model.togglePinned(app.id)
    } label: {
      Label(
        isPinned ? "Unpin" : "Pin",
        systemImage: isPinned ? "pin.slash" : "pin"
      )
    }
    .tint(isPinned ? .gray : .accentColor)
    .accessibilityIdentifier("app.\(app.id).pin-action")
  }
}

private struct AppHostView: View {
  @ObservedObject var model: TerraneIOSModel
  @Environment(\.horizontalSizeClass) private var horizontalSizeClass
  @StateObject private var sidebar = IOSAppSidebarModel()
  @State private var selectedAppID: String
  @State private var isSidebarPresented = false

  init(app: TerraneApp, model: TerraneIOSModel) {
    self.model = model
    _selectedAppID = State(initialValue: app.id)
  }

  var body: some View {
    Group {
      if horizontalSizeClass == .regular {
        NavigationSplitView {
          sidebarContent(showClose: false)
            .navigationTitle("Terrane")
        } detail: {
          appContent
        }
      } else {
        ZStack(alignment: .leading) {
          appContent
          if isSidebarPresented {
            Color.black.opacity(0.28)
              .ignoresSafeArea()
              .onTapGesture {
                withAnimation(.easeInOut(duration: 0.2)) {
                  isSidebarPresented = false
                }
              }
              .accessibilityIdentifier("app-sidebar.scrim")
            sidebarContent(showClose: true)
              .frame(maxWidth: 340)
              .background(.regularMaterial)
              .transition(.move(edge: .leading))
              .zIndex(1)
          }
        }
        .animation(.easeInOut(duration: 0.2), value: isSidebarPresented)
        .toolbar {
          ToolbarItem(placement: .topBarLeading) {
            Button {
              withAnimation(.easeInOut(duration: 0.2)) {
                isSidebarPresented.toggle()
              }
            } label: {
              Image(systemName: "sidebar.left")
            }
            .accessibilityLabel("Show sidebar")
            .accessibilityIdentifier("app-sidebar.toggle")
          }
        }
      }
    }
    .navigationTitle(selectedApp?.name ?? "Terrane")
    .navigationBarTitleDisplayMode(.inline)
    .onChange(of: selectedAppID) {
      sidebar.reset()
    }
  }

  @ViewBuilder
  private var appContent: some View {
    if let app = selectedApp {
      TerraneAppWebView(
        app: app,
        runtime: model.runtime,
        healthAutoSync: model.healthAutoSync,
        sidebar: sidebar,
        permissionHandler: { app, resources in
          await model.requestPermission(app: app, resources: resources)
        }
      )
      .id(app.id)
    } else {
      ContentUnavailableView("App unavailable", systemImage: "square.grid.2x2")
    }
  }

  private var selectedApp: TerraneApp? {
    model.apps.first(where: { $0.id == selectedAppID })
  }

  private func sidebarContent(showClose: Bool) -> some View {
    IOSAppSidebarContent(
      apps: model.orderedApps,
      selectedAppID: selectedAppID,
      section: sidebar.section,
      showClose: showClose,
      onClose: {
        withAnimation(.easeInOut(duration: 0.2)) {
          isSidebarPresented = false
        }
      },
      onSelectApp: { appID in
        selectedAppID = appID
        withAnimation(.easeInOut(duration: 0.2)) {
          isSidebarPresented = false
        }
      },
      onSelectSection: { itemID in
        sidebar.select(itemID)
        withAnimation(.easeInOut(duration: 0.2)) {
          isSidebarPresented = false
        }
      },
      onCreate: {
        sidebar.create()
        withAnimation(.easeInOut(duration: 0.2)) {
          isSidebarPresented = false
        }
      }
    )
  }
}

@MainActor
final class IOSAppSidebarModel: ObservableObject {
  @Published private(set) var section: IOSAppSidebarSection?
  private var actionHandler: (([String: String]) -> Void)?

  func update(_ section: IOSAppSidebarSection?) {
    self.section = section
  }

  func reset() {
    section = nil
    actionHandler = nil
  }

  func setActionHandler(_ handler: @escaping ([String: String]) -> Void) {
    actionHandler = handler
  }

  func select(_ id: String) {
    actionHandler?(["kind": "select", "id": id])
  }

  func create() {
    actionHandler?(["kind": "create"])
  }
}

struct IOSAppSidebarSection: Equatable {
  struct Item: Equatable, Identifiable {
    let id: String
    let title: String
    let systemImage: String?
  }

  let title: String
  let items: [Item]
  let selectedItemID: String?

  static func parse(_ value: Any?) -> IOSAppSidebarSection? {
    guard let value = value as? [String: Any],
      let title = bounded(value["title"], maximum: 80),
      let rawItems = value["items"] as? [[String: Any]],
      !rawItems.isEmpty,
      rawItems.count <= 30
    else {
      return nil
    }
    let items = rawItems.compactMap { raw -> Item? in
      guard let id = bounded(raw["id"], maximum: 80),
        let title = bounded(raw["title"], maximum: 120)
      else {
        return nil
      }
      return Item(
        id: id,
        title: title,
        systemImage: bounded(raw["systemImage"], maximum: 80)
      )
    }
    guard items.count == rawItems.count, Set(items.map(\.id)).count == items.count else {
      return nil
    }
    let selected = bounded(value["selectedItemId"], maximum: 80)
    return IOSAppSidebarSection(
      title: title,
      items: items,
      selectedItemID: selected.flatMap { candidate in
        items.contains(where: { $0.id == candidate }) ? candidate : nil
      }
    )
  }

  private static func bounded(_ value: Any?, maximum: Int) -> String? {
    guard let value = value as? String else { return nil }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.count <= maximum else { return nil }
    return trimmed
  }
}

private struct IOSAppSidebarContent: View {
  let apps: [TerraneApp]
  let selectedAppID: String
  let section: IOSAppSidebarSection?
  let showClose: Bool
  let onClose: () -> Void
  let onSelectApp: (String) -> Void
  let onSelectSection: (String) -> Void
  let onCreate: () -> Void

  var body: some View {
    List {
      if showClose {
        HStack {
          Text("Terrane")
            .font(.headline)
          Spacer()
          Button(action: onClose) {
            Image(systemName: "xmark")
          }
          .accessibilityLabel("Close sidebar")
        }
      }
      if let section {
        Section {
          ForEach(section.items) { item in
            Button {
              onSelectSection(item.id)
            } label: {
              HStack {
                Label(item.title, systemImage: item.systemImage ?? "circle")
                Spacer()
                if item.id == section.selectedItemID {
                  Image(systemName: "checkmark")
                    .foregroundStyle(.tint)
                }
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("app-sidebar.section.\(item.id)")
          }
        } header: {
          HStack {
            Text(section.title)
            Spacer()
            Button(action: onCreate) {
              Image(systemName: "plus")
            }
            .accessibilityLabel("Create \(section.title) item")
          }
        }
      }
      Section("Apps") {
        ForEach(apps) { app in
          Button {
            onSelectApp(app.id)
          } label: {
            Label(app.name, systemImage: NativeAppIconCatalog.systemName(for: app))
              .frame(maxWidth: .infinity, alignment: .leading)
              .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .foregroundStyle(app.id == selectedAppID ? Color.accentColor : Color.primary)
          .accessibilityIdentifier("app-sidebar.app.\(app.id)")
        }
      }
    }
    .listStyle(.sidebar)
  }
}
