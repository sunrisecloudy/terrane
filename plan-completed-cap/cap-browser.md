# Capability: `browser` — headless page rendering as a recorded effect

New crate `rust/crates/terrane-cap-browser/`, namespace `browser`. What
[net-v2](cap-net-v2.md) cannot do: execute a page's JavaScript. Most of the
modern web is JS-rendered, so an agent (or app) that needs "what does this
page actually say/look like" needs a browser, not an HTTP client. Cloudflare
ships this as Browser Rendering; for Terrane it is also the PDF/screenshot
generator ([cap-media.md](cap-media.md) deliberately excluded video/PDF).

## Design

Same recorded-effect shape as net-v2: decide validates purely, the edge
renders once, the **result** is the fact.

```jsonc
// browser.render {app, request_json}
{
  "url": "https://example.com/dashboard",
  "output": "text",            // text | html | screenshot | pdf
  "waitMs": 2000,              // settle time after load; max 15000
  "viewport": {"w": 1280, "h": 800},
  "sensitiveHeaders": []       // same redact-on-record contract as net-v2
}
```

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `browser.render` | → `Decision::Effect(Effect::BrowserRender)` — recorded |
| Resource | `browser.render(requestJson)` | recorded; also `browser.peek(requestJson)` as the transient (live, unrecorded) variant, mirroring `net.get` |
| Event | `browser.rendered` | `{app, request_key, url, output, status, body_kind, body/body_hash, size, mime, title}` — text/html ≤ 256 KiB inline, else blob CAS; screenshot/pdf **always** blob refs per [cap-blob.md](cap-blob.md) |
| (reacts) | `app.removed` | drop the app's renders |

`request_key` = sha256 of the canonical request (net-v2 convention); state
keyed by it, replay folds the recorded result and never launches a browser.

## Engine (edge)

- **macOS host: headless `WKWebView`** — zero extra install, and the codebase
  already drives WKWebView (remember: read `WKError` userInfo, not
  `localizedDescription`, for real errors). PDF via
  `createPDF`, screenshot via `takeSnapshot`.
- **CLI/web hosts:** use system Chrome/Chromium headless if present
  (`--headless=new --dump-dom / --screenshot / --print-to-pdf`); a typed
  `BrowserUnavailable` error names the missing engine otherwise. No bundled
  Chromium (hundreds of MB against "start small").
- `output: "text"` = DOM innerText post-settle (the agent-friendly default —
  smallest tokens, highest signal); `html` = serialized DOM.

## Security

- URL policy identical to net-v2: http/https only, cloud-metadata IP blocked,
  localhost allowed (documented choice).
- **Ephemeral profile per render**: no cookies/storage persist between
  renders, no shared session with the user's real browser — a rendered page
  can never see the user's logged-in state. Authenticated rendering (inject a
  named connection's cookies) is a listed decision, default **off**.
- JS runs inside the OS webview sandbox; the page gets no bridge — nothing is
  injected except the settle/extract script.
- Grant `browser` (namespace-v1), prompt: "Load and execute web pages in a
  hidden browser." Render time capped ⇒ no long-lived pages.

## Limits

`waitMs` ≤ 15 s, total render ≤ 30 s; text/html ≤ 8 MiB (blob past 256 KiB);
screenshot/pdf ≤ 32 MiB; 30 renders/hour per app (renders are expensive);
viewport ≤ 3840×2160.

## Implementation plan

1. **Interface:** `Effect::BrowserRender { app, request: String }` in
   `terrane-cap-interface::abi`.
2. **Crate:** request parse/validate/canonicalize (+ redaction), fold,
   `rendered_event()` constructor, doc, describe (host+path, no query string —
   net-v2 philosophy).
3. **Edge:** mac WKWebView runner (in the mac host's native layer, surfaced
   through the existing host-services seam) + chromium-headless fallback
   runner in `terrane-host`; blob offload path.
4. **`APP_API.md`:** `ctx.resource.browser.render/peek` + "summarize this
   page" agent example.
5. **Tests:** engine (validation, fold/replay, request_key stability); e2e
   against a local JS-rendering test page served from the loopback test
   server (asserts net.fetch sees empty div, browser.render sees the
   JS-inserted text — the reason this cap exists); screenshot/pdf smoke on
   mac, `#[ignore]`d where no engine exists.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Interactive automation (click/type flows — that is
[mcp-client](cap-mcp-client.md) + a Playwright-style MCP server, or the
applescript cap driving a real browser), persistent sessions/logins, crawling
(one URL per render; loops belong to the app), video capture.

## Decisions to confirm

- **WKWebView-first with chromium fallback** — recommend as specced (zero
  install on the primary platform) — alternative: chromium-only (uniform
  engine, mandatory dependency).
- **Authenticated renders via connection cookies** — recommend defer
  (privacy blast radius; needs [cap-oauth-connections.md](cap-oauth-connections.md)
  cookie kind) — alternative: ship in v1 behind its own grant.
- **30/hour rate limit** — recommend as specced — alternative: unlimited
  local (it's the user's CPU), limits only via grants.
