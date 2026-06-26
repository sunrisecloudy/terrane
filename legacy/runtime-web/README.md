# Runtime Web Target

Codex should implement the shared WebView runtime here.

Runtime responsibilities:

- App launcher.
- App registry.
- Manifest/package validation.
- Sandboxed generated app execution.
- `AppRuntime.call` bridge object.
- Fixed `AppRuntime.on` event subscriptions for runtime/app lifecycle signals.
- Per-mount bridge nonce and `MessagePort` context binding.
- WebKit, Android, and WebView2 native host dispatch through a runtime-owned `{ appId, mountToken, request }` envelope.
- Permission checks.
- Storage-prefix enforcement.
- Network policy preflight.
- Per-minute bridge/network/log budget checks.
- Debug console.
- Local browser mock host for development.
- Development-only `window.__APP_RUNTIME_DEVTOOLS__` helpers for Codex snapshot/query/bridge-log/console/storage/core/reset inspection.
- User-customizable theme, edited in the Engine Room and applied to every mounted app.

Generated apps must remain build-free HTML/CSS/vanilla JS packages.

## Custom theming

`theme.js` owns a single user theme (preset or custom palette), persisted in
`localStorage` under `terrane.theme.v1`. The Engine Room "Appearance" section
edits it; the runtime applies it to **every** app it mounts.

Apps are themed through CSS custom properties, not package edits. To be
themeable, a generated app should drive its colours from these design tokens
(the bundled examples already do):

| Token | Meaning |
| --- | --- |
| `--bg` | App page background |
| `--panel` | Cards, panels, inputs |
| `--text` | Primary text |
| `--muted` | Secondary text |
| `--border` | Dividers and outlines |
| `--accent` | Primary buttons and highlights |
| `--danger` | Destructive actions |

The runtime overrides these (plus optional `--accent-strong`, `--warn`,
`--good`, `--rose`, `--soft`, `--shadow`) on the app's document root via
`element.style.setProperty`, which is allowed under the strict generated-app CSP
and outranks the app's own `:root`/`prefers-color-scheme` rules. Tokens the user
leaves unset fall through to the app's own defaults, so a "system" theme keeps
each app's light/dark palette.

Theme delivery:

- **At mount** (web/`srcdoc` path): the active tokens are baked into the
  injected bootstrap and applied before first paint.
- **Live** (already-mounted app): a `runtime.theme_changed` runtime event is
  pushed over the app's bridge port and re-applied without a reload.

### Native hosts

The browser reference host and any shell that mounts apps via `srcdoc` (today:
the browser, Android, and Windows/WebView2 hosts) apply the theme exactly as
above. Shells that mount apps through an `app-runtime://` frame instead of
`srcdoc` — currently the WebKit macOS/iOS and Linux hosts — inject their own
per-platform bootstrap, which does **not** yet carry an `applyTheme` handler.

On those `app-runtime` shells the Engine Room theme editor still renders and
still persists the user's choice, but **mounted apps will not reflect it yet**:
the at-mount bake never runs (no `srcdoc`), and the live `runtime.theme_changed`
event reaches the frame but is dropped because the native `appRuntimeUserScript`
has no handler for it. So on macOS/iOS/Linux the editor is currently a
persisted-but-inert control. Closing native parity means teaching each native
`appRuntimeUserScript` to handle `runtime.theme_changed` and apply tokens via
`style.setProperty`/`removeProperty` (mirroring the web bootstrap), and ideally
gating or labelling the editor on those hosts until then.
