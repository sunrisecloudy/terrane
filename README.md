# Terrane

**A local-first home for personal apps on your Mac.**

Terrane brings useful everyday tools into one native macOS app while keeping
your data under your control. Open an app, get something done, and keep working
even when you are offline. Accounts and sync are optional.

> **Recommended release: Terrane for macOS v0.3.0**
>
> [Download Terrane v0.3.0 for Apple silicon][release] from GitHub Releases.
> This is the recommended version for everyday use and replaces the older
> unsigned preview builds.

[release]: https://github.com/sunrisecloudy/terrane/releases/tag/v0.3.0

## Why use Terrane?

- **Local-first by default.** Core apps remain useful without an account,
  network connection, or cloud service.
- **One place for personal tools.** Switch between apps from the native macOS
  sidebar instead of managing a collection of unrelated utilities.
- **Private, app-scoped storage.** Apps receive only the capabilities and data
  boundaries declared in their manifests.
- **Optional sync.** Enable Premium sync explicitly when you want it; signing
  in is never required for local apps.
- **Built to be extended.** App Builder and the documented app API make it
  possible to create new Terrane apps without changing the core.

## What is included in v0.3.0?

Terrane ships with a growing catalog of built-in apps, including:

- **Todo** for simple local task management
- **Health** for meal, nutrition, history, and calendar views
- **Spending** for local invoice and expense organization
- **Visual Intake** for bringing selected photos into private workflows
- **Chat** for conversations with registered local models
- **Photobooth, Scribe, Pixel Paint, Search Notes, OS Monitor,** and more
- **Control Room** for a read-only view of installed apps, capabilities,
  permissions, models, and runtimes
- **App Builder** for creating and validating your own Terrane apps

The `v0.3.0` macOS release adds native Premium sign-in, optional sync
boundaries, the Health and Spending experiences, native photo intake, and
refined app icons in the sidebar.

## Install on macOS

Terrane `v0.3.0` currently supports Apple silicon Macs running macOS 13 or
newer.

1. Open the [Terrane v0.3.0 release][release].
2. Download the macOS Apple silicon archive.
3. Unzip it and move **Terrane** to `/Applications`.
4. Open Terrane from Finder.

The public build is signed with an Apple Developer ID and distributed with
Hardened Runtime. Release downloads are notarized before publication so macOS
can verify their origin.

## Privacy and accounts

Terrane starts in local mode. You can browse, open, edit, and run local apps
without signing in. If you choose to enable sync, authentication happens in
native host UI and credentials are not exposed to generated apps, web views, or
app event logs.

For Developer ID distribution, Premium sync uses Google sign-in. Apple does
not make its Sign in with Apple entitlement available to Developer ID apps
distributed outside the Mac App Store.

## For developers

Terrane is a Rust workspace with native Apple hosts and web-based app bundles:

```text
apps/                       Built-in personal app bundles
host/macos/                 Native AppKit + WebKit macOS host
host/ios/                   Native SwiftUI iPhone host
host/apple/                 Shared Apple Premium session client
rust/crates/terrane-core/   Deterministic local-first engine
rust/crates/terrane-host/   Host services, CLI, C ABI, preview, sync, and MCP
```

The core follows one rule: requests enter through the host, become events, and
replay into the same state.

```text
argv ──▶ terrane-host ──▶ Request ──▶ terrane-core ──▶ [Event] ──▶ State
                                           │                         │
                                           └── persist log ──────────┘
```

Build and test the Rust workspace with the shared cache wrapper:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
```

Generate the macOS Xcode project:

```sh
cd host/macos
xcodegen generate
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the system model,
[docs/APP_API.md](docs/APP_API.md) for the app API, and
[host/macos/README.md](host/macos/README.md) for native build, signing, and
release instructions.

## Versioning

The current recommended release is `v0.3.0`. Feature releases advance one
minor version at a time: `v0.4.0`, `v0.5.0`, and so on until Terrane is ready
for `v1.0.0`.

Bug-fix releases use the patch number, such as `v0.3.1`.
