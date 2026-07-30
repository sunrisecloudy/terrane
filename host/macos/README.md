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

The checked-in Health app demonstrates the image-model path. On macOS its
Photos button calls the generic `window.terrane.pick(...)` bridge; the host
receives only the selected transferable, normalizes it to a metadata-stripped
JPEG with a 2048-pixel longest edge, and imports it through Health's existing
blob grant. Health previews that canonical blob with `blobUrl` and asks
`ctx.resource.model` to attach the same verified blob, avoiding a second
upload. Ordinary JPEG/PNG/WebP HTML file input remains available on web and
non-native hosts and is normalized in the page before `blob.put`.

OpenCode with `opencode-go/kimi-k2.6` is the default vision path; users can
select another configured OpenCode multimodal model or Codex. Nutrition values
are presented as editable estimates with confidence, assumptions, and a
non-medical disclaimer. KV stores only settings, nutrition records, and the
logical blob name; bytes remain in the per-app blob CAS. Model calls default to
a 120-second timeout, configurable with `TERRANE_MODEL_TIMEOUT_MS`.

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

## Native Premium account

Premium sign-in is optional native host chrome. If no Premium URL is supplied,
the account control is hidden and Terrane remains a local-first unsigned app
host. Signing in is never required to discover, open, edit, or run local apps.

The account button and provider chooser are outside the app-content
`WKWebView`. Sign in with Apple uses `ASAuthorizationController`; Sign in with
Google uses the official `GoogleSignIn` and `GoogleSignInSwift` package pinned
in `project.yml`. Provider credentials are sent directly to the Premium native
exchange endpoint. After capturing Google's ID token, the host immediately
calls the SDK's `signOut()` so Google OAuth credentials are removed from the
SDK Keychain.

The host integrates the shared `TerranePremiumSession` Swift package under
`host/apple/TerranePremiumSession`; `PremiumSessionClient` is the single
account/session boundary used by Apple hosts:

- `/auth/native/challenge`
- `/auth/apple/native/exchange`
- `/auth/google/native/exchange`
- `/account/session/refresh`
- `/account/session/logout`

The service uses the Premium `{ok,result}` envelope.
Credential-bearing requests require HTTPS; plain HTTP is accepted only for
`localhost`, `127.0.0.1`, or `::1` development servers.
Session responses must contain an access token, rotating refresh token, and an
account/user id. Only the rotating Premium refresh token is stored, as a
non-synchronizing `kSecClassGenericPassword` item with
`AfterFirstUnlockThisDeviceOnly` accessibility. The access token, account
metadata, and provider ID/auth codes remain in host memory.

Refresh is single-flight, rotates the stored token before publishing a new
in-memory access token, retries a host-owned request once after a 401, preserves
the refresh token when offline, and deletes it on revoked/reused sessions.
Logout always clears local credentials even if the server is unreachable.
Provider linking reuses the challenge/exchange endpoints with an in-memory
bearer token. Account deletion is wrapped by the shared module's lifecycle
hooks; a successful authoritative deletion clears all local session state. No
deletion route is invented here because it is not part of the native server
contract above.

Premium catalog requests are made by this host boundary with an access token
when one is available. Tokens are never injected into remote pages,
`TerraneBridge`, generated-app APIs, URL fragments, local storage, or app
events. Premium dashboard content therefore receives no native bearer
credential.

### Provider configuration

The checked-in `Configs/PremiumAuth.xcconfig` supplies Terrane's three public
production OAuth identifiers:

```xcconfig
TERRANE_GOOGLE_CLIENT_ID = <macOS OAuth client ID>
TERRANE_GOOGLE_SERVER_CLIENT_ID = <web/server client ID>
TERRANE_GOOGLE_REVERSED_CLIENT_ID = <reversed macOS client scheme>
```

`GIDServerClientID` therefore uses the web/server client ID, while the callback
scheme is derived only from the macOS client ID. For a separate development
Google Cloud project, copy `Configs/PremiumAuth.xcconfig.example` to the
gitignored `Configs/PremiumAuth.local.xcconfig`; its values override the
checked-in defaults because it is included last.

Do not add Google client secrets, Apple private keys, SaaS tokens, or any real
user credential to this repository or an app bundle. Configure the server
client ID as the backend ID-token audience.

## Build

Requires `xcodegen` (`brew install xcodegen`), Xcode, and `cargo`. The project
is defined by `project.yml`; the `.xcodeproj` is generated (gitignored). A
pre-build phase builds `libterrane_host.a` in a dedicated static target and
stages llama's non-TLS HTTP helper archive. The target force-loads the Rust
archive by explicit path, so the finished app has neither a repository dylib
dependency nor a Homebrew OpenSSL dependency.
When `TERRANE_CAP_BUNDLE_DIR` points to a verified release bundle directory, a
post-build phase embeds its index, verifying key, and 41 signed native workers
under `TerraneHost.app/Contents/Resources/capabilities`. Alternatively,
`TERRANE_CAP_SIGNING_KEY_HEX` packages the workers from source. Development
builds without either setting retain the one-release static fallback. Every
build also embeds the checked-in first-party app bundles under
`TerraneHost.app/Contents/Resources/apps`, so a normal Finder or LaunchServices
launch does not depend on a repository working directory or shell environment.

```sh
cd host/macos
xcodegen generate
xcodebuild -project Terrane.xcodeproj -scheme TerraneHost -configuration Debug \
  -derivedDataPath ./.derived CONFIGURATION_BUILD_DIR="$PWD/build/Debug" \
  CODE_SIGNING_ALLOWED=NO build
```

The unsigned command above is valid for local-only use. Provider sign-in
requires a signed build because AuthenticationServices and Keychain access are
code-signing capabilities.

### Signed Premium builds

Public feature releases use minor-version increments: `v0.3.0`, `v0.4.0`,
`v0.5.0`, and so on until the product is ready for `v1.0.0`. The Git tag,
release title, and `MARKETING_VERSION` must agree.

For development or App Store distribution, enable Sign in with Apple for
`com.terrane.host` in the Apple Developer portal and use a provisioning profile
containing that entitlement. The default checked-in entitlements declare Sign
in with Apple and put the private default app Keychain access group first, as
required by Google Sign-In on macOS. Keep the group app-specific; do not
substitute a shared Keychain group.

Generate, archive, and sign with the checked-in public production OAuth
configuration. If present, `Configs/PremiumAuth.local.xcconfig` is applied
automatically for a developer-only override:

```sh
cd host/macos
xcodegen generate
xcodebuild -project Terrane.xcodeproj -scheme TerraneHost \
  -configuration Release DEVELOPMENT_TEAM=<team-id> CODE_SIGN_STYLE=Automatic \
  ENABLE_HARDENED_RUNTIME=YES archive \
  -archivePath "$PWD/build/TerraneHost.xcarchive"
```

Apple does not support Sign in with Apple for Developer ID distribution. The
`DeveloperID` configuration therefore uses
`TerraneHostDeveloperID.entitlements` and omits the Apple provider from the
sync sign-in sheet while retaining Google sign-in and local/offline use:

```sh
cd host/macos
xcodegen generate
xcodebuild -project Terrane.xcodeproj -scheme TerraneHost \
  -configuration DeveloperID \
  TERRANE_DEVELOPMENT_TEAM=<team-id> \
  TERRANE_DEVELOPER_ID_PROFILE=<profile-name> \
  -destination 'generic/platform=macOS' archive \
  -archivePath "$PWD/build/TerraneHost-developer-id.xcarchive"
```

Verify the signed product before notarization:

```sh
APP="build/TerraneHost.xcarchive/Products/Applications/TerraneHost.app"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements :- "$APP"
plutil -p "$APP/Contents/Info.plist"
```

Development/App Store acceptance requires
`com.apple.developer.applesignin`; Developer ID acceptance requires that
entitlement to be absent. Both require `keychain-access-groups`, `GIDClientID`,
`GIDServerClientID`, the reversed Google URL scheme, Hardened Runtime, and the
expected signing identity. Also confirm that no token-like value appears in the
Info.plist, entitlements, generated project, app resources, or logs.

## Run

The app needs to find the workspace log; built-in app UI bundles ship inside
the application. Development overrides remain available:

- `TERRANE_HOME` — the workspace dir (holds `log.bin`); default `~/.terrane`.
- `TERRANE_REPO` — optional repo root for live-editing `apps/<id>/<manifest.ui>`
  ahead of the packaged copy.

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
TERRANE_HOME=~/.terrane ( cd ../.. && cargo run -q -p terrane-host --bin terrane -- log )
# → app.added + kv.set todo/seq, kv.set todo/item:1; NO host.* records
TERRANE_HOME=~/.terrane ( cd ../.. && cargo run -q -p terrane-host --bin terrane -- replay )
# → replay ok
```

The C ABI itself is covered by Rust tests in `rust/crates/terrane-host`
(`cargo test -p terrane-host --test abi`); this host is the GUI layer over it.

Focused Premium account verification:

```sh
cd host/macos
xcodegen generate
xcodebuild test -quiet -project Terrane.xcodeproj \
  -scheme TerraneHostE2ETests -destination 'platform=macOS' \
  -only-testing:TerraneHostE2ETests/PremiumNativeAuthTests \
  CODE_SIGNING_ALLOWED=NO

swift test --package-path ../apple/TerranePremiumSession
```
