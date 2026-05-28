# Runtime Web Target

Codex should implement the shared WebView runtime here.

Runtime responsibilities:

- App launcher.
- App registry.
- Manifest/package validation.
- Sandboxed generated app execution.
- `AppRuntime.call` bridge object.
- Per-mount bridge nonce and `MessagePort` context binding.
- WebKit native host dispatch through a runtime-owned `{ appId, mountToken, request }` envelope.
- Permission checks.
- Storage-prefix enforcement.
- Network policy preflight.
- Per-minute bridge/network/log budget checks.
- Debug console.
- Local browser mock host for development.

Generated apps must remain build-free HTML/CSS/vanilla JS packages.
