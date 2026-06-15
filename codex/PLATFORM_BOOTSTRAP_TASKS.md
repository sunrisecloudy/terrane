# Platform Bootstrap Tasks

## iOS/macOS Swift

- Create WKWebView app.
- Copy runtime and examples into bundle resources.
- Add `WebBridge` to receive JSON bridge messages.
- Add `ForgeCoreBridge` wrapping `forge_core_handle_command`.
- Implement storage in Application Support.
- Implement `notification.toast` as runtime fallback first.
- Add simulator smoke test.

## Android Kotlin

- Create WebView Activity.
- Copy runtime and examples into assets.
- Add bridge with strict JSON dispatch.
- Add JNI wrapper for Forge FFI.
- Implement storage in internal app files.
- Add emulator smoke test.

## Windows C++/WinRT

- Create WebView2 host.
- Load runtime from resources/local folder.
- Implement `WebMessageReceived` dispatch.
- Load Forge FFI DLL.
- Store app data under LocalAppData.
- Add launch smoke test.

## Linux C/GTK4

- Create GTK4 window with WebKitGTK WebView.
- Load runtime from resources.
- Use user content manager script message handler.
- Load Forge FFI shared library.
- Store app data under XDG data path.
- Add launch smoke test.

## Forge Server

- Route HTTP requests through Forge core commands.
- Implement `/health`, `/bridge`, and `/events/drain`.
- Add contract tests using bridge fixtures.
