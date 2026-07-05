# Tauri v2 — the native-surface checklist

Desktop/mobile app framework whose ~30 official plugins are the most complete
enumeration of "what native surface do real apps ask for". Its capability
system (default-deny, per-window permission grants in JSON) is also
philosophically aligned with Terrane's grants.

## Official plugin list (v2)

autostart, barcode-scanner, biometric, clipboard-manager, CLI args, dialog
(open/save/message), **deep-link** (custom URI schemes + verified App/Universal
Links), filesystem, geolocation, global-shortcut, haptics, HTTP client,
localhost, logging, NFC, notification, opener (open with default app), OS
info, persisted-scope, positioner, process, shell, single-instance, SQL
(sqlx), store (KV), stronghold (encrypted DB), updater, upload, websocket,
window-state.

Security model: all dangerous commands blocked by default; capabilities files
grant per-window/per-platform permissions (`nfc:allow-scan`,
`biometric:allow-authenticate`); remote content gets a narrower grant set.

## Mapping to Terrane plans

| Tauri plugin | Terrane |
| --- | --- |
| clipboard, dialog, notification, opener | native cap (shipped ops) |
| save dialog, clipboard-read, screenshot, tray, global-shortcut, window-state | [../cap-native-v2.md](../cap-native-v2.md) |
| geolocation | [../cap-geolocation.md](../cap-geolocation.md) |
| barcode/camera, haptics-adjacent capture | [../cap-capture.md](../cap-capture.md) (mobile ops group exists) |
| HTTP, websocket, upload | [../cap-net-v2.md](../cap-net-v2.md), [../cap-stream.md](../cap-stream.md) |
| SQL, store, stronghold | relational_db, kv, crypto (shipped) |
| updater | [../cap-app-update.md](../cap-app-update.md) |
| logging | [../cap-telemetry.md](../cap-telemetry.md) |

## What it exposed

- **deep-link** → [../cap-deep-links.md](../cap-deep-links.md) (scheme, file
  associations, share target — delivery via `common.receive`).
- **autostart** — listed as an agent-readiness follow-up (the scheduler's
  long-running host assumes it).
- **biometric** — follow-up: one native op, natural gate for crypto's vault.
- **NFC / haptics / barcode** — mobile-host territory; the native cap's
  `mobile` operations group is the placeholder, the host itself is the
  missing piece (named residual, not a cap).

## Sources

- https://v2.tauri.app/plugin/
- https://v2.tauri.app/security/capabilities/
- https://v2.tauri.app/plugin/deep-linking/
- https://github.com/tauri-apps/plugins-workspace
