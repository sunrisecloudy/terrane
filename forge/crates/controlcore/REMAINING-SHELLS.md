# Remaining DevControlPlane shell migrations (B7)

Migrated in B6/B7 (call `control.*` via `handle_command` / `control-invoke`):

| Shell | Migrated operations |
|---|---|
| macOS | `compare_snapshot`, `json_matches_subset`, `package_validate`, `package_hashes`, `backup_validate`, `backup_content_hash`, `generate_token`, `sign_payload`, `verify_signature` |
| reference-host | same as macOS |
| iOS | `compare_snapshot`, `json_matches_subset`, `backup_validate`, `backup_content_hash`, `generate_token` |
| Linux | `compare_snapshot`, `json_matches_subset`, `package_validate`, `backup_validate`, `backup_content_hash`, `generate_token` |
| Windows | `compare_snapshot`, `json_matches_subset`, `package_validate`, `backup_validate`, `backup_content_hash`, `generate_token` |
| Android | `compare_snapshot`, `json_matches_subset` |

Phase C bridge delegation (via `bridge.validate_envelope` / `bridge.validate_network_request`):

| Shell | Migrated |
|---|---|
| macOS | envelope + network (B6 baseline) |
| iOS | network (`PlatformNetwork`); envelope pending WebBridge gate |
| Linux | pending `webkit_host` / `platform_network` |
| Windows | pending `WebBridge` / `PlatformNetwork` |
| Android | envelope + network |

Phase D package lifecycle (`package.rollback_version`, `package.set_status`, `quota.auto_quarantine`):

| Shell | Migrated |
|---|---|
| macOS | full (`PlatformPackageLifecycle`) |
| Windows | pending (local SQL fallback remains) |
| Linux / Android | N/A (no rollback/quarantine routes) |

Still local (future commits):

| Operation | Linux | Windows | Android |
|---|---|---|---|
| Static HTML query / a11y audit | `dev_control_plane.c` | `DevControlPlane.cpp` | WebView bridge only |
| Smoke-test step matching | bundled smoke runner | static test runner | bundled smoke runner |
| Fault/network/dialog mocks | effect mock routes | mock routes | mock routes |
| Core-event replay assertions | core debug routes | core debug routes | core debug routes |
| `control.sign_payload` / `verify_signature` (E13) | package sign routes (local hash) | package sign routes | — |

Wire pattern per shell:

1. Build `libforge_ffi` with `forge-core` `control` feature (already default for `forge-ffi`).
2. Delegate pure JSON work to `control.*` / `bridge.*` commands; keep HTTP listener + SQLite I/O in the shell.
3. Delete the duplicated algorithm once golden vectors pass on that shell.