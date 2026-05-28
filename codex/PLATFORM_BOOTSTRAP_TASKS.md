# Platform Bootstrap Tasks

## iOS/macOS Swift

- Create WKWebView app.
- Copy runtime and examples into bundle resources.
- Add `WebBridge` to receive JSON bridge messages.
- Add `ZigCoreBridge` wrapping `core_step_json`.
- Implement storage in Application Support.
- Implement `notification.toast` as runtime fallback first.
- Add simulator smoke test.

## Android Kotlin

- Create WebView Activity.
- Copy runtime and examples into assets.
- Add bridge with strict JSON dispatch.
- Add JNI wrapper for Zig core.
- Implement storage in internal app files.
- Add emulator smoke test.

## Windows C++/WinRT

- Create WebView2 host.
- Load runtime from resources/local folder.
- Implement `WebMessageReceived` dispatch.
- Load Zig DLL.
- Store app data under LocalAppData.
- Add launch smoke test.

## Linux C/GTK4

- Create GTK4 window with WebKitGTK WebView.
- Load runtime from resources.
- Use user content manager script message handler.
- Load Zig shared library.
- Store app data under XDG data path.
- Add launch smoke test.

## Server Zig

- Directly import core modules.
- Implement `/health` and `/core/step`.
- Add contract tests using bridge fixtures.
