# Capability: `model` v2 — sessions, streaming, tool use, multimodal

An **extension of the existing `model` and `local-model` crates**, not a new
namespace. Today `model.ask` is a one-shot recorded call to an agent CLI
(`claude -p` / `codex exec`), and `local-model` already has multi-turn chat —
`local-model.responded` carries `continued: bool` plus `system`, `ok`,
`constraint`, `token_count`, `duration_ms`, with `local-model.chat-cleared` as
the reset. v2 keeps every existing event kind folding byte-for-byte and adds
new kinds beside them (the `net` v2 pattern, see
[cap-net-v2.md](cap-net-v2.md)).

Which crate gets what:

| Feature | `model` (agent CLIs) | `local-model` (local engines) |
| --- | --- | --- |
| Named sessions | new `model.session.*` commands | new, same event shape; implicit per-app chat kept |
| Streaming | yes (CLI `--output-format stream-json` / stdout) | yes (token callback already exists in the engine) |
| Tool use | yes — the headline | v2.1, gated on model quality; format ready |
| Multimodal input | yes (blob refs) | only for vision-capable specs; typed error otherwise |

## Locked decision

**The stream is transient; the completed turn is the fact.** The edge streams
deltas to the UI live over the existing host→shell progress channel — the same
`terrane:bridge:progress` postMessage that already holds an invoke open during
permission elicitation (`host/web/src/js/app_shell.js` `sendBridgeProgress`,
deadline-extension in `terrane_shim.js`; MCP hosts use
`notifications/progress`). The progress frame gains an optional payload
(`{kind: "model.delta", text}`); a bare frame stays a keep-alive, so old shims
keep working. **Only the final completed turn is recorded as an event.** Replay
folds the finished turn and never needs the stream — replay reconstructs what
was said, not how fast it appeared. Nothing about streaming can therefore
break replay-identity.

## Sessions

Aligned with `local-model`'s existing turn shape rather than inventing a new
one. New commands (namespace `model`; `local-model` mirrors them):

| Command | Args | Event |
| --- | --- | --- |
| `model.session.start` | `app, agent, [--system <text>]` | `model.session-started { app, session, agent, system }` — `session` id minted at the edge and recorded once (the `Effect::NewReplicaId` → `replica.initialized` pattern: entropy runs at the edge, replay reads the id back) |
| `model.session.append` | `app, session, prompt_json` | `Effect::ModelTurn` → `model.turned` (below) |
| `model.session.end` | `app, session` | `model.session-ended { app, session }` |

`model.turned` payload: `{ app, session, agent, prompt_json, response, ok,
exit_code, tool_calls, duration_ms }` — a superset of today's
`LocalModelTurn` fields so the two crates' transcripts stay shape-compatible.
`model.ask` remains and is defined as a one-turn anonymous session (still
emits `model.responded`, unchanged). Fold: `ModelState` gains
`sessions: BTreeMap<AppId, BTreeMap<SessionId, Vec<Turn>>>`; `app.removed`
clears it as today. `local-model` adds an optional `session` argument to
`chat`/`chatModel`; its implicit per-app chat (`continued` + `chat-cleared`)
is retained as the anonymous session.

## Tool use

Mid-turn, the model may request Terrane capability calls. This already exists
in spirit: the top-bar agent assist loop (`terrane-cap-agent` + the host MCP)
drives the current app through the host's own MCP tools, each call going
through normal dispatch. v2 pulls that loop to the edge runner:

- The edge advertises granted tools to the agent CLI (claude/codex both accept
  MCP config); each tool invocation is routed through the **normal dispatch
  path as the app principal**, so per-app grants are enforced exactly as if
  the backend had called `ctx.resource.*` itself.
- Each tool call commits **its own ordinary events** (`kv.set`, `net.responded`,
  …) — nothing model-specific in the log. The final `model.turned` records the
  tool-call *trace* (`tool_calls: [{name, args_digest, ok}]`) for audit, not
  the results (they live in their own events).
- A `permission_required` mid-turn holds the turn open via the same
  elicitation flow the shell already runs; deny fails the tool call, not the
  turn.

## Multimodal input

`prompt_json` is either a plain string (back-compat) or
`{"parts": [{"text": "…"}, {"blob": "photos/receipt.jpg"}]}`. Blob parts are
app-scoped blob names resolved through the `blob` capability's folded meta
(hard dependency: [cap-blob.md](cap-blob.md)); the edge reads bytes from the
CAS at call time. The recorded event keeps only the blob `name` + `hash` — the
turn is replayable as a fact without re-reading images. Missing blob ⇒ typed
error in decide (state check), missing CAS bytes ⇒ typed edge error.

## Limits

- ≤ 64 turns per session, ≤ 16 image parts per turn, image part ≤ 16 MiB
  (CAS-side check), ≤ 8 tool calls per turn (loop guard), tool trace args
  digest is sha256 — all typed errors named in `doc.rs`.
- Session ids and turn order are log order; concurrent appends to one session
  serialize through the single-writer lock like every command.

## Security / permissions

- Tool use grants nothing new: the app's existing grants are the ceiling, and
  the permission prompts name the capability being called, not "the model".
- Streaming deltas go only to the shell frame that issued the invoke (same
  origin/nonce discipline as the bridge today); they are never persisted.
- Prompts recorded in events are the app's own data — same stance as today's
  `model.responded`; `describe()` truncates like the existing impls.

## Implementation plan

1. **Interface:** add `Effect::ModelTurn { app, session, agent, prompt_json,
   tools }` and a `StreamSink` handle on the runner context (no-op default so
   CLI/tests need no channel).
2. **`terrane-cap-model`:** session commands, `turned_event()` /
   `session_started_event()` constructors, fold + describe, `doc.rs`; keep
   `model.ask`/`model.responded` untouched.
3. **`terrane-cap-local-model`:** optional `session` arg on `chat`/`chatModel`;
   emit the aligned turn shape; implicit chat unchanged.
4. **Edge:** `ModelTurn` arm — spawn agent CLI in streaming mode, forward
   deltas to `StreamSink`, run the tool loop through dispatch-as-app, build
   the final event. Web shell: payload on `terrane:bridge:progress`; MCP:
   `notifications/progress`.
5. **Blob parts:** resolve via `blob` state + CAS read (after cap-blob step 3).
6. **App surface:** `ctx.resource.model.session*` methods; `APP_API.md`.
7. **Tests:** engine tests `terrane-core/tests/cap/model.rs` (session fold,
   replay identity, turn-shape parity with local-model, blob-ref validation);
   e2e `terrane-host/tests/cap/model.rs` with a fake agent binary on `PATH`
   (default-run); real-CLI streaming/tool-use cases `#[ignore]` like today.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v2)

Direct API-key HTTP providers (below), tool use on local models (v2.1),
audio/video parts, cross-app sessions, session forking/branching, embeddings
(its own surface — the hybrid-search work adds `local-model.embed` separately
and this plan must rebase on it if it lands first).

## Decisions to confirm

- **Direct HTTP API providers (Anthropic/OpenAI keys)** — *recommendation:*
  defer; keep the provider surface CLI agents + local engines, and when a real
  need lands, build it on [cap-oauth-connections.md](cap-oauth-connections.md)
  key storage + `net` v2's redact-on-record so keys never enter the log.
  *Alternative:* ship now behind a `model.provider.register` command — rejected
  as a second secrets story before oauth-connections exists.
- **Where sessions live for local models** — *recommendation:* mirror the
  `model.session.*` commands in `local-model` with the shared turn shape, keep
  the implicit per-app chat as the anonymous session. *Alternative:* a single
  shared `session` namespace over both caps — rejected: it would be the first
  cross-capability command coupling, against the registry rule.
- **Delta transport** — *recommendation:* extend `terrane:bridge:progress`
  with an optional payload (proven holding mechanism, zero new plumbing).
  *Alternatives:* SSE endpoint per invoke (more moving parts, better for very
  long streams); WebSocket (overkill for v2).
