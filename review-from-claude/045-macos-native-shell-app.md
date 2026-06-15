# 045 macOS native shell app review

## Slice goal

Add a native AppKit shell for the macOS host: a Finder-scale window, sidebar app
catalog, titlebar sidebar toggle, and runtime-web host mode that mounts bundled
apps through `window.TerraneRuntimeHost` while preserving the existing
Forge-backed WKWebView bridge.

## Diff reviewed

Working-tree slice before commit.

## Files changed

- `native/macos/Sources/TerraneHostMac/App.swift`
- `native/macos/Sources/TerraneHostMac/NativeShellViewController.swift`
- `native/macos/Sources/TerraneHostMac/WebHostView.swift`
- `native/macos/Tests/TerraneHostMacTests/NativeHostTests.swift`
- `runtime-web/runtime.js`
- `runtime-web/styles.css`
- `tools/reference-host/test/runtime-web.test.js`

## Commands run

- `swift test` in `native/macos` passed outside the managed sandbox: 23 tests.
- `node --test --test-reporter=dot --no-warnings tools/reference-host/test/runtime-web.test.js` passed: 17 tests.
- `git diff --check` passed for the slice files.

The first sandboxed `swift test` failed because SwiftPM could not write
`~/.cache/clang/ModuleCache`; the same command passed with approved escalation.

## Claude review status

Attempted to run a read-only `claude -p` Opus review for this exact slice, but
the Codex sandbox reviewer rejected it because it would send private working-tree
code to an external service. I did not route around that restriction.

## Local review findings

- `NativeShellViewController.swift` was untracked but required by `App.swift`
  and the tests. Resolution: include it in this slice.
- `WebHostView` called the async runtime `mountApp` through plain
  `evaluateJavaScript`, which could hide rejected mounts. Resolution: switch to
  `callAsyncJavaScript`, report failures back to the native workspace header,
  and add a runtime-web rejection regression for unknown app ids.
- Catalog load errors were collapsed into an empty app list. Resolution:
  distinguish catalog load failures from a valid empty catalog and log skipped
  malformed manifests.
- No unresolved Claude review blocker was found for macOS launch, reopen,
  sidebar, or persistence. The prior macOS Forge-FFI/persistence findings are
  marked closed by reviews `044` and `FINAL`.

## Follow-ups

- Manual packaged-app check remains useful: launch the macOS app, select apps
  from the sidebar, toggle the sidebar, and verify close/reopen behavior from
  Dock/Finder before screenshot capture.
- Broader review findings in `review/078-*` about FFI handle safety and secrets
  plumbing are cross-cutting Forge runtime work, not part of this app-shell
  slice.
