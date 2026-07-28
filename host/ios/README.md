# Terrane iOS host

This is the supported SwiftUI iOS host for the current Terrane architecture.
It does not use the archived Forge-era mobile runtime.

## Trust boundary

- Local app frontends load from the application bundle through the
  `terrane-app://` scheme.
- On iPhone and Apple Silicon simulator builds, app calls cross the same
  checked-in `terrane_host.h` C ABI used by the macOS host and execute in the
  current Rust host/runtime.
- `TerranePremiumSession` is the canonical host-owned Apple-platform module
  under `host/apple/TerranePremiumSession`. It owns all Premium HTTP and
  Keychain access for both macOS and iOS.
- The WKWebView bridge contains only `terrane.invoke`. Premium access tokens,
  refresh tokens, Apple credentials, and Google credentials are never added to
  JavaScript, URL parameters, cookies, user defaults, or generated-app storage.
- Premium sign-in is optional. Signing out or deleting a Premium account does
  not remove local app data.

## Configuration

Copy `Config/Local.xcconfig.example` to `Config/Local.xcconfig` and replace the
placeholder values locally. Never commit OAuth identifiers from a private
environment or any credentials.

The build consumes Google Sign-In 9.x through Swift Package Manager. The iOS
OAuth redirect scheme must match `REVERSED_GOOGLE_CLIENT_ID`. Enable the Sign in
with Apple capability for the application identifier in the Apple developer
portal before signing a device/archive build.

Premium endpoints used by the host:

- `POST /auth/native/challenge`
- `POST /auth/apple/native/exchange`
- `POST /auth/google/native/exchange`
- `POST /account/session/refresh`
- `POST /account/session/logout`

Account deletion is intentionally completed in the system browser at the
host-owned Premium deletion page after the local session is cleared. No Premium
session is transferred to a WKWebView.

## Build and test

```sh
cd host/ios
xcodegen generate
xcodebuild -project TerraneIOS.xcodeproj -scheme TerraneIOS \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build test
```

Device and simulator builds compile and link `libterrane_host.a` through the
repository's shared Cargo cache:

```sh
xcodebuild -project TerraneIOS.xcodeproj -scheme TerraneIOS \
  -destination 'generic/platform=iOS' CODE_SIGNING_ALLOWED=NO build
```
