# host/macos — Terrane macOS host

A native AppKit + WKWebView app that runs a Terrane app's UI and bridges it to
terrane-core over the [`terrane-ffi`](../../terrane-core/crates/terrane-ffi) C
ABI. The first non-Rust host; the same shape (FFI + thin shell) is how iOS /
Android / Windows hosts will work.

```
WKWebView (apps/<id>/index.html + terrane.invoke shim)
   │ window.webkit.messageHandlers.terrane.postMessage({verb, args})
   ▼
TerraneBridge (WKScriptMessageHandlerWithReply)
   │ terrane_host_run(handle, app, argv)        ← terrane-ffi C ABI
   ▼
libterrane_ffi.a  ──▶  terrane-core: dispatch("host.run", …)
   │ output string → reply settles the JS Promise
   ▼
WKWebView re-renders
```

Every UI action is a `host.run` → recorded `kv.*` → replayable, exactly like the
CLI. The app id is fixed at launch, so a page can only act as its own app.

## Build

Requires `xcodegen` (`brew install xcodegen`), Xcode, and `cargo`. The project is
defined by `project.yml`; the `.xcodeproj` is generated (gitignored). A pre-build
phase builds `libterrane_ffi.a` and the target links it.

```sh
cd host/macos
xcodegen generate
xcodebuild -project Terrane.xcodeproj -scheme TerraneHost -configuration Debug \
  -derivedDataPath ./.derived CONFIGURATION_BUILD_DIR="$PWD/build/Debug" \
  CODE_SIGNING_ALLOWED=NO build
```

## Run

The app needs to find (a) the workspace log and (b) the app's UI bundle:

- `TERRANE_HOME` — the workspace dir (holds `log.bin`); default `~/.terrane`.
- `TERRANE_REPO` — repo root, so it can resolve `apps/<id>/index.html`.

```sh
# register the app once (any TERRANE_HOME)
( cd ../../terrane-core && TERRANE_HOME=~/.terrane \
    cargo run -q -p terrane-cli -- app add todo Todo --source "$PWD/../apps/todo" )

# launch the todo app
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
