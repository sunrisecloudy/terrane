# Capability: `deep-links` — URL scheme, file associations, share target

Mostly host-edge work plus a thin recorded surface (folded into the existing
`native`/`app` machinery — a new crate only if registration facts don't fit
either; recommend extending `terrane-cap-app` with link registration facts).
This is how the OS gets INTO Terrane apps: `terrane://` links, "Open with
Terrane", and the macOS share sheet. Every delivery lands through
[cap-interop.md](cap-interop.md)'s required `common.receive` — deep links are
just the second sender after email, which is why this plan is thin.

## Surface

| Piece | Design |
| --- | --- |
| URL scheme | `terrane://open/<app>` (open in shell), `terrane://send/<app>?kind=…&payload=…` (deliver via `common.receive("link", payload)`), and item URIs `terrane://app/<app>/item/<itemId>` (open the app focused on the item — resolved via required `common.get`, [primitive-item.md](primitive-item.md)). Host registers the scheme (mac: Info.plist `CFBundleURLTypes`; web shell: `registerProtocolHandler` where applicable). |
| File associations | Manifest gains `"fileTypes": [{ext, mime}]`. The mac host registers declared types (`CFBundleDocumentTypes`); "Open with Terrane" imports the file's bytes to the blob CAS ([cap-blob.md](cap-blob.md)) and delivers `common.receive("blob", {name, hash, size, mime})`. Multiple claimants → the interop picker. |
| Share target | mac Services / share-sheet extension: "Send to Terrane" → picker over `inbox` apps → `common.receive`. |
| Recorded facts | `app.link.registered {app, kind: scheme-route\|filetype, spec}` on install (folded from the manifest — deterministic); each delivery is the ordinary `interop.called` event, nothing new. |

## Security

Links and shared files are **untrusted input**: payloads are delivered only
through `common.receive` (never arbitrary verbs — `terrane://` cannot invoke
`kv.set` or any other verb directly); first delivery from a given source kind
to a given app raises a confirm prompt via the existing elicitation flow;
payload size caps mirror interop's (64 KiB inline, blob for files). URL
payloads are percent-decoded, never shell-interpreted.

## Limits

Scheme payload ≤ 64 KiB; file import ≤ blob cap (64 MiB); one prompt-free
delivery path only after user confirmation is recorded as a grant.

## Implementation plan

1. Manifest `fileTypes` parsing + `app.link.registered` fold in
   `terrane-cap-app`; validation of specs.
2. mac host: URL scheme + document types + Services entry; routing into the
   shell (open) and interop dispatch (send), with the picker for ambiguity.
3. Web host: route `/#open/<app>` parity + `registerProtocolHandler`
   best-effort.
4. CLI: `terrane open <url>` for testing the full path headlessly.
5. `APP_API.md`: fileTypes + the `link`/`blob` payload kinds for
   `common.receive`.
6. Tests: engine (registration folds, replay); e2e via `terrane open`
   (scheme → interop.called → target kv state), file-import path with a temp
   file.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

iOS/Android (no mobile host yet), Universal Links / domain-verified links
(needs web-publish domains first — cross-link
[cap-web-publish.md](cap-web-publish.md)), drag-and-drop onto the dock icon
(native-v2 follow-up), Windows/Linux registration.

## Decisions to confirm

- **Crate home** — recommend extending `terrane-cap-app` (registration is an
  app-catalog fact) — alternative: standalone `terrane-cap-links` crate.
- **First-delivery confirmation scope** — recommend per (source-kind → app)
  grant — alternative: per-link confirmation always (safer, noisier).
