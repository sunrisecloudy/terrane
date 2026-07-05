# Capability: `mcp` — apps consuming external MCP servers

New crate `rust/crates/terrane-cap-mcp-client/`, namespace `mcp`, registered
in `default_registry`. Lets an app's backend call tools on **external** MCP
servers (a Linear MCP, a filesystem MCP, a vendor's hosted MCP) as ordinary
recorded effects.

**The boundary, stated once:** `rust/crates/terrane-host/src/mcp.rs` is
Terrane's **own MCP server** — external agents (and the `agent` cap's shell
assistants) drive *Terrane* through it. This cap is the **opposite
direction**: Terrane apps as MCP *clients* of other people's servers. The two
share nothing but the protocol; neither imports the other.

## Design

Two layers, mirroring [cap-oauth-connections.md](cap-oauth-connections.md):

1. A **host-level registry** of named MCP connections (operator-defined, like
   credentials — apps cannot add servers).
2. **Per-call effects**: `mcp.call` → `Decision::Effect` → the edge does the
   MCP round-trip → the result is recorded. Replay folds `mcp.called` and
   **never spawns a server or opens a session** — the standard effect replay
   story ([cap-net-v2.md](cap-net-v2.md)).

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `mcp.connect` | args `name, transport_json` → pure validate + redact; emits `mcp.connected` (registry entry; the live session is edge-lazy, opened on first call) |
| Command | `mcp.disconnect` | pure; emits `mcp.disconnected`; edge tears down any live session on fold |
| Command | `mcp.call` | args `app, connection, tool, args_json` → `Decision::Effect(Effect::McpCall)` — **recorded** |
| Event | `mcp.connected` | `{name, transport_json_redacted}` |
| Event | `mcp.disconnected` | `{name}` |
| Event | `mcp.called` | `{app, connection, tool, args_json_redacted, result_kind, result, result_is_base64, result_hash, result_size, is_error, called_at}` |
| Resource | `mcp.call(connection, tool, argsJson)` | routes to the **recorded** command — external tool calls are side-effecting by assumption, so there is no transient variant of `call` |
| Resource | `mcp.tools(connection)` | `Decision::TransientEffect` — live `tools/list` round-trip, returned to the app, **never recorded** (discovery output is not replay-stable and not a fact worth keeping) |
| (reacts) | `app.removed` | drop the app's recorded-call state |

`transport_json`:

```jsonc
{ "stdio": { "cmd": "npx", "args": ["-y", "@upstash/mcp"], "env": { "API_KEY": {"$secret": "upstash"} } } }
{ "http":  { "url": "https://mcp.example.com/mcp", "headers": { "authorization": {"$secret": "example.header"} } } }
```

Auth flows through [cap-oauth-connections.md](cap-oauth-connections.md)
`$secret` references — resolved by the edge at session-open, **marker recorded
verbatim**, never the value. Header redaction otherwise identical to net-v2.

Fold keeps registry `name → transport_redacted` plus
`app → callKey → RecordedCall` where
`callKey = sha256(canonical {connection, tool, args})` — the net-v2
request-key pattern, so identical calls overwrite.

## Results, sizes, and the blob CAS

Per the [cap-net-v2.md](cap-net-v2.md)/[cap-blob.md](cap-blob.md) convention:
result content ≤ 256 KiB inlines in the event; larger or binary content
(image/audio content blocks) goes to the blob CAS with `result_kind: "blob"`
+ hash/size and a `__mcp__/<callKey>` blob link. Hard cap **32 MiB**. The
recorded `result` is the MCP `content` array as canonical JSON; `is_error`
mirrors the protocol flag and still folds — a tool error is a fact.

## Edge behavior

- `Effect::McpCall` arm in `EdgeRunner::run`: get-or-open the session for the
  connection (initialize handshake, honoring `MCP_PROTOCOL_VERSION` from
  `terrane-api`), issue `tools/call`, shape the result, return
  `[mcp.called]` (+ `blob.stored` on offload).
- stdio sessions: spawned child, reused across calls, killed after 5 min
  idle or on `mcp.disconnected` fold. Spawn failure → `mcp.called` with
  `is_error` + message (a failed attempt is recorded, matching
  [cap-email.md](cap-email.md)).
- http sessions: reuse net-v2's HTTP client and its SSRF stance (`http(s)`
  only, deny `169.254.169.254`, private/loopback allowed).
- Timeout 60 s default / 300 s max per call, from `args`-level `timeoutMs`?
  No — from a `transport_json` default plus per-call override in the command
  args. No automatic retries (net-v2 stance).
- CLI host: works — a call spawns the stdio server for the process lifetime;
  only session *reuse* is a long-running-host luxury.

## Security & permissions

- **Per-app, per-connection grants**: grant resource `mcp:<name>` through the
  existing `auth` prompts — "This app wants to call tools on **linear**
  (external MCP server)." Same per-name shape as
  [connection grants](cap-oauth-connections.md); wholesale `mcp` grants do
  not exist.
- Registry writes (`mcp.connect`/`disconnect`) are operator-only, same trust
  surface as `connection.define`.
- **Args/results may carry secrets** — apply the net-v2 redaction philosophy:
  `$secret` markers in transport/env are recorded verbatim; a per-call
  `sensitiveArgs` list of JSON pointers (e.g. `"/token"`) redacts those arg
  values to `«redacted»` before the event is written. Results are recorded
  as returned — the doc says plainly that calling a tool which echoes secrets
  writes them to the log, and that `sensitiveArgs` is the app's tool for the
  request side.
- `describe()` prints connection, tool, result size, `is_error` — never args.
- stdio `cmd` is an arbitrary-process launch: `mcp.connect` for stdio
  transports is called out in the admin console with the exact command line,
  the same severity as a native-cap grant.

## Limits (documented in `doc.rs`)

- ≤ 16 connections per home; ≤ 128 KiB args; result ≤ 32 MiB (offload above
  256 KiB); ≤ 60 calls/min per app per connection (typed error).

## Implementation plan

1. **Interface:** add `Effect::McpCall { app, connection, tool, args: String }`
   to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-mcp-client`:** transport parse/validate/redact,
   `callKey` canonicalization, decide (connect/disconnect/call/tools), fold,
   `called_event()` constructor, describe, `doc.rs`.
3. **Edge:** `terrane-host/src/mcp_client.rs` — session manager (stdio spawn
   + http), JSON-RPC client (initialize/tools-list/tools-call), `$secret`
   resolution via the [connection resolver](cap-oauth-connections.md), blob
   offload; `McpCall` + transient `tools` arms in `EdgeRunner::run`.
4. **App surface:** `APP_API.md` — `ctx.resource.mcp.call/tools`, manifest
   `resources: ["mcp:<name>"]`; CLI `terrane mcp connect/ls/rm/call`.
5. **Tests:** engine (`terrane-core/tests/cap/mcp.rs`): callKey stability,
   redaction (`$secret` verbatim + `sensitiveArgs`), fold/replay identity,
   app.removed. E2e (`terrane-host/tests/cap/mcp.rs`): a tiny in-repo stdio
   MCP echo server (a test binary speaking initialize/tools/call) + a
   loopback http MCP — call round-trip, is_error fold, blob offload, grant
   denial; default-run. Real third-party servers `#[ignore]` (reason:
   external effect).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Serving MCP (that is `terrane-host/src/mcp.rs`), MCP resources/prompts/
sampling/elicitation (tools only), app-defined connections, streaming partial
tool output (see [cap-stream.md](cap-stream.md) for feeds), result-side
redaction transforms, connection health monitoring.

## Decisions to confirm

- **Resource `mcp.call` records (no transient call)** — *recommend: recorded
  only* (external tools mutate the world; an unaudited mutation path is a
  hole) — *alternative:* a `mcp.callTransient` for provably-read-only tools;
  cheaper log, but "read-only" is the server's unverifiable claim.
- **Tool discovery transient and unrecorded** — *recommend: as specced*
  (catalogs churn; recording them is log noise) — *alternative:* record
  `mcp.tools_listed` snapshots for offline discovery in replayed state.
- **callKey overwrite semantics (net-v2 pattern)** — *recommend: as specced*
  (state stays bounded; the log keeps full history anyway) — *alternative:*
  append-keyed by seq for in-state call history; grows state, duplicates the
  log's job, worsens the [cap-compaction.md] pressure that
  [cap-stream.md](cap-stream.md) already creates.
- **`sensitiveArgs` JSON-pointer redaction** — *recommend: include in v1*
  (args are the likeliest secret leak after headers) — *alternative:* defer
  and rely on `$secret` only; simpler, but literal secrets in args then land
  in the log verbatim.
