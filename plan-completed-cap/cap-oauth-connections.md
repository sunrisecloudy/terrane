# Capability: `connection` — named secrets + OAuth token acquisition

New crate `rust/crates/terrane-cap-connection/`, namespace `connection`,
registered in `default_registry`. The host-level store of **named credentials**
("github", "smtp-default", "openai") and the machinery that keeps OAuth2
tokens fresh. This is the doc [cap-net-v2.md](cap-net-v2.md),
[cap-email.md](cap-email.md), [cap-mcp-client.md](cap-mcp-client.md),
[cap-webhook.md](cap-webhook.md) (MAC keys) and [cap-stream.md](cap-stream.md)
lean on.

## The contract (crisp)

1. **Secrets never appear in events, state, or `describe()` — ever.** Events
   record only facts *about* connections (`defined`, `authorized`, `revoked`).
   The secret bytes live in a host-side store the core never opens.
2. **`{"$secret": "<name>"}` resolves at the edge.** Anywhere a capability's
   request JSON accepts a string, this reserved shape (reserved and rejected
   by net-v2 today — this cap un-rejects it) is substituted by the
   `EdgeRunner` immediately before the effect executes. The **recorded event
   keeps the marker verbatim**, never the resolved value — stronger than
   `«redacted»`, and it keeps net-v2's `requestKey` stable across secret
   rotation.
3. **Resolution requires a grant.** An app may reference `$secret: "github"`
   only if it holds a grant on connection `github`; otherwise the effect fails
   with a typed error naming the missing grant (the same
   permission-prompt flow `auth` runs for resources).

## Secret store (edge)

**OS keychain via the `keyring` crate, with an encrypted-file fallback.**

- Primary: one keychain entry per secret, service `terrane`, account
  `<home_id>/<name>/<field>` — macOS Keychain / Secret Service / Windows
  Credential Manager for free.
- Fallback (headless Linux, CI): `$TERRANE_HOME/secrets.enc`, sealed with the
  existing `terrane-cap-crypto` primitives (XChaCha20-Poly1305 `seal`/`open`,
  Argon2id KDF) under a host master password or a `0600` key file. **Reuse
  crypto's primitives, not its vault**: the crypto cap is the *per-app,
  app-facing* vault (master password per app, session keyring); connections
  are *host-level, edge-facing*. Two stores, one cipher suite.
- The store is a `terrane-host` module (`secret_store.rs`), sibling of the
  blob CAS — opened by the edge, invisible to the core.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `connection.define` | args `name, kind, config_public_json` → pure; emits `connection.defined`. **Secret material does not travel in this command** — see entry paths below |
| Command | `connection.remove` | pure; emits `connection.removed`; edge deletes store entries on fold |
| Command (host-only) | `connection.mark_authorized` | dispatched by the edge after a successful OAuth exchange/refresh; emits `connection.authorized` / `connection.refreshed` |
| Event | `connection.defined` | `{name, kind, config_public_json}` — kind `apiKey \| oauth2 \| smtp` |
| Event | `connection.authorized` | `{name, scopes, expires_at}` — facts only, no tokens |
| Event | `connection.refreshed` | `{name, expires_at}` |
| Event | `connection.removed` | `{name}` |
| Resource | `connection.list()` / `connection.stat(name)` | pure reads: kind, authorized?, expires_at — an app sees only connections it is granted |

`config_public_json` by kind — public parts only: `apiKey` `{}` (the key is
store-only, field `key`); `oauth2` `{auth_url, token_url, client_id, scopes,
use_pkce}` (`client_secret` and access/refresh tokens store-only); `smtp`
`{host, port, starttls, username}` (`password` store-only). Fold keeps
`name → ConnMeta {kind, config_public, authorized, expires_at}`.

## Secret entry paths (how bytes reach the store without touching the log)

- CLI: `terrane connection set <name> [--field key]` — host reads the value
  from stdin/prompt, writes the store **at the edge**, then dispatches
  `connection.define` with public facts. The secret never enters a `Request`.
- Web/mac admin console (the same trusted surface that approves permission
  prompts): a form that POSTs to a host-local admin route → store → define.
- Apps have **no write path**. Defining connections is an operator act.

## OAuth2 flow (edge)

Auth-code + PKCE, run through the host's web shell:

1. Operator hits "Authorize" in the admin console (or `terrane connection
   authorize <name>`, which prints the URL).
2. Host builds the provider auth URL with `redirect_uri =
   http://<host>/oauth/callback/<name>` — a route on the same long-running
   listener [cap-webhook.md](cap-webhook.md) uses — plus a random `state`.
3. Callback: edge exchanges the code at `token_url`, writes tokens to the
   store, dispatches `connection.mark_authorized` → `connection.authorized`.
4. **Refresh on demand at the edge:** when a resolver pulls an oauth2
   connection whose token is expired (or < 60 s from it), it refreshes
   synchronously, updates the store, and dispatches `mark_authorized` →
   `connection.refreshed`. Consumers never see stale tokens; the log sees
   only the fact and the new `expires_at`.

CLI host: `authorize` runs a one-shot loopback listener for the callback
(standard native-app flow), so OAuth works without the web host.

## Resolver (the piece other caps call)

`terrane-host/src/secret_store.rs` exposes
`resolve(app, json) -> Result<json>`: walks the value, replaces every
`{"$secret": "<name>"}` (and `{"$secret": "<name>.<field>"}` for multi-field
kinds) after checking the app's grant on `<name>`, refreshing oauth2 tokens as
above. `EdgeRunner::run` calls it for `HttpRequest`
([net-v2](cap-net-v2.md)), `EmailSend` ([email](cap-email.md)), `McpCall`
([mcp-client](cap-mcp-client.md)), and stream opens
([stream](cap-stream.md)). Resolved values exist only in edge memory.

## Security & permissions

- Per-app grants: connection `X` surfaces as grant resource `connection:X`
  through the existing `auth` permission prompts — "This app wants to use
  your **github** credentials." Granting `connection` wholesale is not a
  thing; grants are per name.
- `describe()` prints names and kinds only. MCP event dumps therefore never
  contain secret material (they only see events).
- Store reads are edge-only; there is deliberately **no**
  `ctx.resource.connection.get(name)` returning the secret — apps consume
  secrets only via `$secret` substitution inside effects.

## Limits (documented in `doc.rs`)

- ≤ 64 connections per home; `name` ≤ 64 chars, `[a-z0-9-_]`; secret value
  ≤ 64 KiB per field.

## Implementation plan

1. **Crate `terrane-cap-connection`:** manifest, decide (define/remove +
   host-only mark_authorized), fold, events, describe, `doc.rs`; grant
   resource per-name integration with `auth`.
2. **Store:** `terrane-host/src/secret_store.rs` — `keyring` backend,
   encrypted-file fallback on `terrane-cap-crypto` primitives, `resolve()`.
3. **Wire the resolver** into `EdgeRunner::run` and **lift net-v2's
   `$secret` rejection** (the reserved shape now resolves; net-v2 event
   format unchanged, as designed).
4. **OAuth edge:** callback route (web/mac listener + CLI one-shot), PKCE
   exchange, refresh-on-demand, `mark_authorized` dispatch; admin console
   form + `terrane connection set/authorize/rm/ls`.
5. **Tests:** engine (`terrane-core/tests/cap/connection.rs`): fold/replay
   identity, grant checks, marker-verbatim recording. E2e
   (`terrane-host/tests/cap/connection.rs`): file-fallback store round-trip,
   `$secret` resolution into a loopback net-v2 request, loopback OAuth
   provider (auth-code + refresh) — default-run; real-keychain and
   real-provider cases `#[ignore]` (reason: OS/session state, external
   effect).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Device-code and client-credentials OAuth grants, secret sync across replicas
(secrets are per-machine by design; re-authorize on each device), secret
versioning/rotation history, an app-readable secret API, browser-extension
autofill.

## Decisions to confirm

- **OS keychain primary, encrypted file fallback** — *recommend: as specced*
  (`keyring` crate; best-available protection per platform) — *alternatives:*
  file-only on crypto primitives (uniform, weaker on mac); keychain-only
  (breaks headless).
- **Marker recorded verbatim (not `«redacted»`)** — *recommend: verbatim
  `$secret` marker* (stable `requestKey`, self-documenting events) —
  *alternative:* normalize to `«redacted»`; loses the which-connection fact.
- **Per-name grants (`connection:X`) vs one `connection` grant** —
  *recommend: per-name* (credentials differ wildly in blast radius) —
  *alternative:* namespace-wide grant; one prompt, far too coarse.
- **Refresh-on-demand only (no background refresher)** — *recommend: on
  demand* (no timer machinery; consumers always check) — *alternative:*
  proactive refresh loop in long-running hosts; smoother p99, more parts.
