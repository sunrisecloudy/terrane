# Terrane

**Your tools. Your Mac. Your data.**

Terrane is a local-first home for the everyday apps you use to plan, create,
organize, and understand your life. Everything lives in one native Mac app,
works without an account, and stays useful when you are offline.

> **[Download Terrane v0.3.0 for macOS][download]**
>
> For Apple silicon Macs running macOS 13 or newer. Developer ID-signed,
> Apple-notarized, and ready for everyday use.

[release]: https://github.com/sunrisecloudy/terrane/releases/tag/v0.3.0
[download]: https://github.com/sunrisecloudy/terrane/releases/download/v0.3.0/Terrane-0.3.0-macos-arm64.dmg

## One app, many useful tools

Terrane ships with 17 built-in app bundles. Highlights in the native sidebar
include:

| What you want to do | Apps to get you started |
| --- | --- |
| Plan your days | Todo, Tomorrow |
| Look after yourself | Health, BMI Calculator |
| Understand your spending | Spending |
| Capture and create | Photobooth, Scribe, Pixel Paint |
| Find and explore | Search Notes, Chat, Visual Intake |
| Stay in control | Password Manager, OS Monitor, Control Room |
| Make something new | App Builder |

Open a tool, do the work, and move on. There is no account wall and no cloud
setup between you and the local apps.

## Local-first by design

- **Useful offline.** Core apps keep working without a network connection.
- **No sign-in required.** Terrane starts in local mode; an account is optional.
- **App-scoped access.** Each app receives only its declared capabilities and
  data boundaries.
- **Sync when you choose.** Premium sync is an explicit opt-in, not a
  prerequisite.
- **Extensible without surrendering control.** App Builder and the documented
  app API let you create tools that follow the same boundaries.

Native capability workers are delivered from the same immutable release only
when needed and are accepted only when their hashes match Terrane's signed
capability index.

## Get started

1. [Download the Terrane v0.3.0 DMG][download].
2. Open it and drag **Terrane** into **Applications**.
3. Launch Terrane and choose an app from the sidebar.

For an independent integrity check, download `SHA256SUMS` from the
[v0.3.0 release page][release] and compare it with the DMG before opening it.
Intel Macs are not currently supported.

## What is new in v0.3.0?

- Health experiences for meals, nutrition, history, calendar, and insights
- Spending tools for local invoice and expense organization
- Native photo selection and the Visual Intake workflow
- Optional native Premium sign-in and sync boundaries
- Refined app icons and a cleaner native sidebar

This is the recommended release and replaces the older unsigned preview
builds.

## Privacy

Terrane starts in local mode. You can browse, open, edit, and run local apps
without signing in. If you enable sync, authentication stays in trusted native
host UI; credentials are not exposed to generated apps, web views, or app
event logs.

See the [macOS release runbook](docs/RELEASING_MACOS.md),
[privacy disclosure](PRIVACY.md), [security policy](SECURITY.md), and
[changelog](CHANGELOG.md).

## For developers

Terrane is open source, built around a deterministic Rust core, native Apple
hosts, and portable web-based app bundles:

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
[host/macos/README.md](host/macos/README.md) for native build and signing
details.

## Versioning

The current recommended release is `v0.3.0`. Feature releases advance one
minor version at a time: `v0.4.0`, `v0.5.0`, and so on until Terrane is ready
for `v1.0.0`.

Bug-fix releases use the patch number, such as `v0.3.1`.
