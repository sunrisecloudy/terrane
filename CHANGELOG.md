# Changelog

All notable end-user changes to Terrane are documented here.

## 0.3.0 - 2026-07-30

- Added native, explicitly opt-in Premium authentication and sync boundaries.
- Added the Health experience with meal, nutrition, history, calendar, and
  insight views.
- Added the Spending app for local invoice and expense organization.
- Added native photo selection and the Visual Intake workflow.
- Added the native iPhone host and shared Apple Premium session client.
- Refined app icons in the macOS sidebar.
- Added a dedicated Developer ID build that retains local/offline use and
  Google Premium sign-in without requesting the unsupported Sign in with Apple
  entitlement.
- Updated the public product README, installation guidance, and release
  versioning cadence.

## 0.2.0 - 2026-07-25

- Rebuilt the production macOS host around AppKit, WebKit, and the Rust Terrane
  core.
- Added fourteen built-in local-first apps and the native App Builder flow.
- Added 42 independently packaged and signed native capability workers.
- Added GUI-owned authenticated MCP attachment and native permission handling.
- Added a production Apple-silicon local release pipeline with Developer ID
  signing, hardened runtime, Apple notarization, Gatekeeper verification,
  checksums, release manifests, and verified GitHub upload.
- Added a production application icon, privacy disclosure, installation guide,
  release runbook, and release notes.
- Added an explicitly unsigned GitHub preview path for testing before
  Developer ID distribution credentials are available.
- Reduced the initial macOS download by stripping release symbols and moving
  signed native capability workers to exact-hash, on-demand release assets.
- Added a hard 40 MiB DMG size budget to local preview and production
  publishers.
