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
            .navigationTitle(app.name)
            .navigationBarTitleDisplayMode(.inline)
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
          .navigationTitle(app.name)
          .navigationBarTitleDisplayMode(.inline)
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
  let app: TerraneApp
  @ObservedObject var model: TerraneIOSModel

  var body: some View {
    TerraneAppWebView(
      app: app,
      runtime: model.runtime,
      healthAutoSync: model.healthAutoSync,
      permissionHandler: { app, resources in
        await model.requestPermission(app: app, resources: resources)
      }
    )
  }
}
