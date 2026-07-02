# host/macos — Terrane macOS host

A native AppKit + WKWebView app switcher that runs Terrane app UIs and bridges
them to terrane-core over the
[`terrane-host`](../../rust/crates/terrane-host) C ABI. The first non-Rust
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
   │ terrane_host_run(handle, app, argv)        ← Terrane host C ABI
   ▼
libterrane_host.a  ──▶  terrane-core: dispatch(runtime run, …)
   │ output string → reply settles the JS Promise
   ▼
WKWebView re-renders
```

Every UI action runs the app's manifest-declared backend runtime, then records
ordinary resource events such as `kv.*` for replay. The app id is selected by
the native shell, so a page can only act as the currently loaded app.

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
pre-build phase builds `libterrane_host.a` and the target links it.

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
# launch on the landing page with a native sidebar switcher
TERRANE_HOME=~/.terrane TERRANE_REPO="$PWD/../.." \
  build/Debug/TerraneHost.app/Contents/MacOS/TerraneHost

# optionally open an initial app directly
TERRANE_HOME=~/.terrane TERRANE_REPO="$PWD/../.." \
  build/Debug/TerraneHost.app/Contents/MacOS/TerraneHost todo
```

Without an app id argument the host opens the shared landing page (the same
page `terrane-web` serves at `/`), rendered by `terrane_home_page` from the
C ABI with the natively discovered catalog inlined. Card clicks navigate to
`terrane-app://<id>/frame/`; the navigation delegate routes them through
native selection so the bridge, sidebar, and source editor follow. The
sidebar's Home entry returns to it.

## Verify

Add a todo in the window, then confirm the GUI session only produced ordinary
events and persisted (the data survives a relaunch because the FFI opens a
file-backed core, not in-memory):

```sh
TERRANE_HOME=~/.terrane ( cd ../../rust && cargo run -q -p terrane-host --bin terrane -- log )
# → app.added + kv.set todo/seq, kv.set todo/item:1; NO host.* records
TERRANE_HOME=~/.terrane ( cd ../../rust && cargo run -q -p terrane-host --bin terrane -- replay )
# → replay ok
```

The C ABI itself is covered by Rust tests in `rust/crates/terrane-host`
(`cargo test -p terrane-host --test abi`); this host is the GUI layer over it.
