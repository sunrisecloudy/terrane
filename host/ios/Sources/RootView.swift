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
      NavigationStack {
        PremiumAccountView(
          controller: model.premiumAccount,
          configuration: model.configuration
        )
      }
      .tabItem {
        Label("Account", systemImage: "person.crop.circle")
      }
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
        List(model.apps) { app in
          NavigationLink(value: app.id) {
            Label {
              VStack(alignment: .leading) {
                Text(app.name)
                Text(app.id)
                  .font(.caption)
                  .foregroundStyle(.secondary)
              }
            } icon: {
              Image(systemName: systemIcon(app.icon))
                .frame(width: 28)
            }
          }
          .accessibilityIdentifier("app.\(app.id)")
        }
      }
    }
    .navigationTitle("Terrane")
    .navigationDestination(for: String.self) { id in
      if let app = model.apps.first(where: { $0.id == id }) {
        TerraneAppWebView(app: app, runtime: model.runtime)
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
    switch model.runtime.availability {
    case .embedded:
      EmptyView()
    case .unavailable(let message):
      Text(message)
        .font(.caption)
        .foregroundStyle(.red)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity)
        .background(.bar)
    }
  }

  private func systemIcon(_ manifestIcon: String) -> String {
    switch manifestIcon {
    case "checklist", "checkmark-square": return "checklist"
    case "heart", "health": return "heart.text.square"
    case "camera": return "camera"
    case "paintbrush": return "paintbrush"
    case "calendar": return "calendar"
    default: return "app"
    }
  }
}
