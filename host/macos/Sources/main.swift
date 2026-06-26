import AppKit

// AppKit app with no storyboard/xib: build the application object and delegate
// programmatically, then run the event loop.
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
