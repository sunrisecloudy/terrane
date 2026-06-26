# Platform Capability Matrix

## 1. Required v0.3 targets

| Capability | reference-host | macOS WKWebView | iOS WKWebView | Android WebView | Windows WebView2 | Linux WebKitGTK | Forge server |
|---|---:|---:|---:|---:|---:|---:|---:|
| load runtime | required | required | required | required | required | required | n/a |
| load sandboxed generated app | required | required | required | required | required | required | n/a |
| app registry | required | required | required | required | required | required | required |
| permissions | required | required | required | required | required | required | required |
| signature verification | required | required | required | required | required | required | required |
| storage bridge | required | required | required | required | required | required | required |
| core.step bridge | required | required | required | required | required | required | required |
| network.request bridge | required | required | required | required | required | required | required |
| dialog.openFile | mock | required | required where platform permits | required | required | required | n/a |
| dialog.saveFile | mock | required | optional | required | required | required | n/a |
| notification.toast | required | required | required | required | required | required | optional |
| capabilities API | required | required | required | required | required | required | required |
| snapshot API | required | dev | dev | dev | dev | dev | required |
| accessibility snapshot | required | dev | dev | dev | dev | dev | n/a |
| screenshot | required | dev | dev | dev | dev | dev | n/a |
| micro-test control | required | dev | dev | dev | dev | dev | required |
| rollback | required | required | required | required | required | required | required |
| migrations | required | required | required | required | required | required | required |

## 2. Reference host is first-class

The reference host is not a throwaway mock. It is the canonical CI/control-plane target for:

- package validation;
- bridge contract tests;
- micro-tests;
- mutation tests;
- snapshot/replay;
- Codex repair loop.

Real native hosts must match reference-host contract behavior unless a documented platform limitation exists.

## 3. Per-platform notes

### iOS

- Uses WKWebView and WebKit JavaScript.
- Treat generated apps as user-created content/config interpreted by the shipped runtime.
- Prefer schema/declarative behavior for App Store-sensitive features.
- `dialog.saveFile` may map to platform share/export flow rather than direct file path access.

### Android

- Use Android WebView with strict JavaScript interface exposure.
- Bridge calls should be JSON messages, not broad Java object access.

### Windows

- Use WebView2; dev control plane can use WebView2 devtools protocol where appropriate.

### Linux

- Use WebKitGTK or Qt WebEngine; document whichever implementation is chosen.

### Server

- Server does not render WebView UI but must run app/package validation, Forge core, snapshots, replay, and bridge contract tests.
