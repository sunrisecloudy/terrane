# Plan: Localization (i18n) for Terrane

## Context

Terrane has no localization today: every user-facing string (host shell chrome,
app UIs, backend/CLI output) is hard-coded English, and there is no way to detect
a user's language or store translations. We want the system and its apps to speak
the user's language, detected automatically, with translations stored once and
reused across every app and every platform (web + macOS).

**Goal.** Detect the language from the request (web `Accept-Language`; macOS
system locale), store translations in a shared *public KV* bucket (so they are
reused across apps and platforms and sync/replay like all other state), expose a
portable `window.terrane` locale/translation API to app UIs, localize the host
chrome and the apps, add a manual language picker, and document how to localize an
app and a capability.

**Initial languages (12).** `en` (default/fallback), `es`, `zh-Hans`, `ar` (RTL),
`pt-BR`, `fr`, `de`, `ja`, `id`, `th-TH`, `ko`, `vi`.

### Decisions (confirmed with the user)
- **Content depth:** translate *everything* — all apps + system chrome across all
  12 languages (not just the reference app).
- **Backend strings:** **UI-only** for v1. App backend/CLI return strings and
  action summaries stay English (the visible Todo UI still fully localizes because
  it renders the `items` JSON, not the confirmation strings). No `ctx.locale` in v1.
- **Manual override:** add a **language picker** in the shell (settings/user menu),
  persisted, overriding the detected language — in addition to header/system
  detection.

## Architecture (one rule preserved)

Translations live in a **shared public KV bucket** — a new generic primitive in
`terrane-cap-kv` — under the key convention `i18n/<code>/<domain>.<key>` where
`<domain>` is `system` (host/shell chrome) or an app id. `en` is the fallback and
the key inventory.

- **Storage / read:** apps read via new read-only resource methods
  `ctx.resource.kv.public(key)` / `publicScan` / `publicAll` / `publicKeys`.
- **Write:** trusted-host-only commands `kv.public.set` / `kv.public.rm` /
  `kv.public.import`, gated in core's `admit_command` exactly like `auth.*`. There
  is **no** public-write resource method, so app backends structurally cannot
  write public data.
- **Negotiation:** a dependency-free leaf crate `terrane-i18n` owns the supported
  list + `Accept-Language`/preferred-list negotiation (single source of truth for
  web, macOS, FFI, CLI).
- **Delivery mirrors the existing theme/document protocol.** The host pushes
  `{locale, dir, messages}` to the app frame over the same channel as
  `terrane:theme` (web postMessage after the nonce-checked hello; macOS
  `window.__terrane_apply`). Apps read it via `window.terrane`.

## Status

### Done (commits `d1af6b16`..`bb88d646`, core/storage half — GLM-5.2)
- `terrane-i18n` leaf crate: `SUPPORTED`, `DEFAULT`, `canonical`,
  `from_accept_language`, `from_preferred_list` (+ unit tests).
- Public KV in `terrane-cap-kv`: `PUBLIC_BUCKET_APP_ID = "__terrane/public"`
  sentinel bucket, `kv.public.set/rm/import` (reuse `kv.set`/`kv.deleted` events
  → fold/replay/storage-sync unchanged), read-only `public*` resource methods,
  hand-rolled flat-JSON parser for `import` (no serde dep in core), `doc.rs`.
- Core `admit_command` gate: `kv.public.*` requires trusted host.
- `terrane-host::import_i18n_dir` edge importer (walks `i18n/system/*.json` +
  `apps/*/i18n/*.json`, sorted/deterministic) + CLI `i18n import` / `i18n
  negotiate`.
- FFI: `terrane_i18n_negotiate`, `terrane_i18n_supported`, `terrane_i18n_import`.
- Seed catalogs for **en, es, zh-Hans** only: `i18n/system/*.json`,
  `apps/todo/i18n/*.json`.
- Tests: negotiation, capability, engine replay/gate, host e2e; `APP_API.md`
  resource table regenerated.

### Remaining
1. **Content:** the other 9 languages (`ar, pt-BR, fr, de, ja, id, th-TH, ko,
   vi`) for `i18n/system` + `apps/todo`, and full catalogs for the other apps
   (`chat`, `pixel-paint`, `photobooth`, `app-builder`, `bmi-calculator`,
   `todo-cli*`), all 12 languages. Automatic seeding of an app's `i18n/` on
   `app.add` (currently import is manual).
2. **`window.terrane` API (both platforms, parity):** `getLocale()`,
   `onLocale(cb)`, `getMessages()`, `onMessages(cb)`, `getDir()` (rtl only for
   `ar`), `t(key, params)` (interpolate `{name}`, fall back to `params.default ??
   key`). Edit `host/web/src/js/terrane_shim.js` and the Swift `shim` string in
   `host/macos/Sources/TerraneBridge.swift` identically.
3. **Web delivery:** read `Accept-Language` in `host/web/src/routes.rs`, negotiate
   via `terrane_host::i18n`, inject `__TERRANE_LOCALE__` / `__TERRANE_MESSAGES__`
   / `__TERRANE_DIR__` / `<html lang dir>` into `shell.html` through
   `shell::response`; push `terrane:locale` to the frame on the hello in
   `app_shell.js`; localize the shell chrome via `data-i18n` + a `shellT()`; add
   the language picker (persisted, re-drives the push). JSON-escape the bundle
   like `premium_url_js`. Logical-CSS pass for RTL.
4. **macOS delivery:** negotiate from `Locale.preferredLanguages` (via
   `terrane_i18n_negotiate`), read the bundle (new `terrane_i18n_bundle` ABI or
   reuse public read via dispatch), extend `applyStateJS`/`__terrane_apply` with
   `locale/messages/dir` (add `jsonObjectLiteral`, escape every key+value),
   localize native chrome ("Untitled", "Code", empty state), picker in the
   sidebar.
5. **App UIs:** wire all apps with `data-i18n`/`data-i18n-attr` + a ~15-line
   `localize()` block reading `getDir()`/`t()` and re-running on `onMessages`.
   Todo is the reference. Graceful fallback when no host (headless/CLI).
6. **Docs:** `docs/APP_API.md` — a "Localization" subsection (getLocale/onLocale/
   getMessages/onMessages/getDir/t, the `i18n/<code>/<domain>.<key>` convention,
   how to ship an app bundle, RTL, fallback, textContent-not-innerHTML security);
   keep the "both hosts expose the same surface" invariant honest.
   `docs/cap-best-practice/` — how a capability exposes/localizes user-facing text
   (lives in public KV; keep `en` complete). "update all the doc to make app and
   cap."

## Critical files
- Core/storage (done): `rust/crates/terrane-i18n/*`,
  `rust/crates/terrane-cap-kv/src/{commands,resources,lib,doc}.rs`,
  `rust/crates/terrane-core/src/lib.rs` (`admit_command`),
  `rust/crates/terrane-host/src/{i18n,cli,ffi}.rs`.
- Shell/UI (remaining): `host/web/src/js/terrane_shim.js`,
  `host/web/src/js/app_shell.js`, `host/web/src/shell.rs`,
  `host/web/src/templates/shell.html`, `host/web/src/routes.rs`,
  `host/macos/Sources/TerraneBridge.swift`,
  `host/macos/Sources/AppDelegate.swift`.
- Apps: `apps/*/index.html`, `apps/*/i18n/<code>.json`, `i18n/system/<code>.json`.
- Docs: `docs/APP_API.md`, `docs/cap-best-practice/*`.

## Verification
- Gate: `scripts/with-cargo-cache.sh cargo test --workspace --locked` and
  `... cargo clippy --workspace --all-targets --locked -- -D warnings` green.
- FFI header drift test passes (new `extern "C"` exports must be declared in the
  checked-in `include/terrane_host.h`).
- Web e2e: run the web host, `GET /apps/todo` with `Accept-Language: ar` → Todo in
  Arabic + `dir="rtl"`; `en`/no header → English/LTR; picker overrides detection.
- macOS: set system language, launch host, open Todo → localized + RTL for Arabic;
  native chrome localized.
- `docs/APP_API.md` generated resource table stays in sync (`UPDATE_DOCS=1 cargo
  test`); shim-parity needle tests (web served shim ⇔ Swift shim) list the same
  `window.terrane` keys.
