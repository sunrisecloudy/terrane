# host/macos — Terrane macOS host

A native AppKit + WKWebView app switcher that runs Terrane app UIs and bridges
them to terrane-core over the
[`terrane-ffi`](../../terrane-core/crates/terrane-ffi) C ABI. The first non-Rust
host; the same shape (FFI + thin shell) is how iOS / Android / Windows hosts
will work.

```
native sidebar (plain UI apps)
   │ selects app id + bundle path
   ▼
WKWebView (apps/<id>/<manifest.ui> + terrane.invoke shim)
   │ window.webkit.messageHandlers.terrane.postMessage({kind:"invoke", verb, args})
   ▼
TerraneBridge (WKScriptMessageHandlerWithReply)
   │ terrane_dispatch(app.add, id, name, --source, path) if needed
   │ terrane_host_run(handle, app, argv)        ← terrane-ffi C ABI
   ▼
libterrane_ffi.a  ──▶  terrane-core: dispatch("host.run", …)
   │ output string → reply settles the JS Promise
   ▼
WKWebView re-renders
```

Every UI action is a `host.run` → recorded `kv.*` → replayable, exactly like the
CLI. The app id is selected by the native shell, so a page can only act as the
currently loaded app.

The sidebar discovers plain HTML UIs from:

- `$TERRANE_REPO/apps/<id>/manifest.json`
- the current working directory's `apps/<id>/manifest.json`
- `$TERRANE_HOME/apps/<id>/manifest.json`
- the app bundle's `Resources/apps/<id>/manifest.json`

`manifest.ui` must point at an existing `.html`/`.htm` file such as `index.html`
or `dist/index.html`. `react:` entries are intentionally skipped; this host only
runs compiled app assets.

## App Builder preview

The injected shim also exposes `window.terrane.preview(files)`. App Builder
passes generated files to the native bridge, which calls
`terrane_preview_create` on the same FFI handle and gets back:

```json
{ "id": "...", "frameUrl": "terrane-preview://<id>/frame/" }
```

The returned URL is loaded in an iframe through `PreviewSchemeHandler`, a
`WKURLSchemeHandler` registered for `terrane-preview` before the `WKWebView` is
created. Requests for `terrane-preview://<id>/frame/` and
`terrane-preview://<id>/frame/<asset>` call `terrane_preview_read_asset`; when
preview documents call `terrane.invoke(verb, ...args)`, the shim detects the
`terrane-preview:` protocol and routes to `terrane_preview_invoke`.

Preview state lives in Rust behind the FFI handle. The macOS host does not write
a temp app bundle or add preview apps to the catalog.

## Build

Requires `xcodegen` (`brew install xcodegen`), Xcode, and `cargo`. The project
is defined by `project.yml`; the `.xcodeproj` is generated (gitignored). A
pre-build phase builds `libterrane_ffi.a` and the target links it.

```sh
cd host/macos
xcodegen generate
xcodebuild -project Terrane.xcodeproj -scheme TerraneHost -configuration Debug \
  -derivedDataPath ./.derived CONFIGURATION_BUILD_DIR="$PWD/build/Debug" \
  CODE_SIGNING_ALLOWED=NO build
```

## Run

The app needs to find (a) the workspace log and (b) local app UI bundles:

- `TERRANE_HOME` — the workspace dir (holds `log.bin`); default `~/.terrane`.
- `TERRANE_REPO` — repo root, so it can resolve `apps/<id>/<manifest.ui>`.

```sh
# launch with a native sidebar switcher
TERRANE_HOME=~/.terrane TERRANE_REPO="$PWD/../.." \
  build/Debug/TerraneHost.app/Contents/MacOS/TerraneHost

# optionally select an initial app
TERRANE_HOME=~/.terrane TERRANE_REPO="$PWD/../.." \
  build/Debug/TerraneHost.app/Contents/MacOS/TerraneHost todo
```

## Verify

Add a todo in the window, then confirm the GUI session only produced ordinary
events and persisted (the data survives a relaunch because the FFI opens a
file-backed core, not in-memory):

```sh
TERRANE_HOME=~/.terrane ( cd ../../terrane-core && cargo run -q -p terrane-cli -- log )
# → app.added + kv.set todo/seq, kv.set todo/item:1; NO host.* records
TERRANE_HOME=~/.terrane ( cd ../../terrane-core && cargo run -q -p terrane-cli -- replay )
# → replay ok
```

The FFI itself is covered by Rust tests in `terrane-core/crates/terrane-ffi`
(`cargo test -p terrane-ffi`); this host is the GUI layer over it.
