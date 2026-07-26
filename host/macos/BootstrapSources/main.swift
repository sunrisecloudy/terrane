import AppKit

final class BootstrapAppDelegate: NSObject, NSApplicationDelegate {
  private var window: NSWindow?
  private var viewController: BootstrapViewController?

  func applicationDidFinishLaunching(_ notification: Notification) {
    do {
      let configuration = try BootstrapConfiguration.resolve()
      let manager = BootstrapManager(configuration: configuration)
      let viewController = BootstrapViewController(manager: manager)
      let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 520, height: 360),
        styleMask: [.titled, .closable, .miniaturizable],
        backing: .buffered,
        defer: false
      )
      window.title = "Terrane"
      window.titlebarAppearsTransparent = true
      window.isMovableByWindowBackground = true
      window.contentViewController = viewController
      window.center()
      window.makeKeyAndOrderFront(nil)
      NSApp.activate(ignoringOtherApps: true)
      self.window = window
      self.viewController = viewController
    } catch {
      let alert = NSAlert(error: error)
      alert.addButton(withTitle: "Quit")
      alert.runModal()
      NSApp.terminate(nil)
    }
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    true
  }
}

let application = NSApplication.shared
application.setActivationPolicy(.regular)
let delegate = BootstrapAppDelegate()
application.delegate = delegate
application.run()
