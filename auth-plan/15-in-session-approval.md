# 15 — In-Session Permission Approval (production)

Status: **planned** (scope locked 2026-07-01). Supersedes the "restart the MCP
server to see a grant" workaround. Builds directly on the permission-request
broker (doc 11) and the runtime gate (doc 07).

## Goal

While a model's MCP session is live, the model hits `permission_required`, a
**human approves without leaving the session**, the grant applies to the **same
live Core**, and the model's retry succeeds — **no server restart**. The
untrusted model can never self-grant.

Two approval front-ends, one live Core:

1. **In-session (primary):** MCP **elicitation** — the approve prompt appears
   inside the model's client (opencode, confirmed to implement
   `elicitation/create` + `notifications/elicitation/complete`, form + URL mode).
2. **Admin console / headless (secondary):** the existing loopback admin surface
   (`host/web/src/admin.rs` approve/deny handlers) hosted **inside the
   `terrane-mcp` process**, approving against the same live Core — for browser
   consoles, non-elicitation clients, and cron/headless operators.

Scope decision (2026-07-01): **build both** (phases 1–4). Concurrency decision:
**enforce an exclusive single-writer lock** — a second writer process fails fast;
approval must go through the live session. This is the mechanism that makes the
live Core authoritative.

## Why this is small at the core (do not re-derive)

- **Approve *is* the grant.** `decide_permission_approve` emits the `granted`
  events itself (`terrane-cap-auth/src/lib.rs:528`); `namespace_granted` flips
  true the moment they fold. No separate `auth.grant` step.
- **The server is already trusted in-process.** `dispatch_on_core` uses
  `Request::trusted_host` (`terrane-host/src/lib.rs:170`);
  `approve_permission_request` already dispatches the approval against the live
  Core (`terrane-host/src/permission.rs:217`).
- **The pending request already exists at the wall.** The invoke path records
  `auth.permission.request` before returning `permission_required`
  (`terrane-host/src/permission.rs:158`), so `requestId` is known and `pending`.
- **The model has no self-grant path.** No grant/approve tool exists;
  `capability_command` refuses `auth.*` and dispatches Public
  (`terrane-host/src/mcp.rs:734,747`); the core gate rejects Public `auth.*`
  (`terrane-core/src/lib.rs:1012`).

So the grant is a **one call** — `approve_permission_request(core, request_id, …)`.
The production work is entirely **transport** (get a human "yes" into the running
process) and **concurrency** (make the live Core the single source of truth).

## Architecture: Core-owning actor, two front-ends

`Core` is `!Send` (owns the QuickJS runtime). To let a concurrent admin listener
mutate the same Core the stdio loop uses, one thread **owns** the Core; all
front-ends submit commands over a channel. This also serializes every write
through one owner (in-process single-writer).

```
                 ┌────────────────────────────────────────────┐
   stdin/stdout  │  stdio front-end (model's MCP session)      │
   ────────────▶ │    - tools/call, elicitation nested-recv    │─┐
                 └────────────────────────────────────────────┘ │  CoreCmd
                 ┌────────────────────────────────────────────┐ │  (mpsc)
   127.0.0.1     │  admin front-end (human console / headless) │─┤
   ────────────▶ │    - GET requests, POST approve/deny        │ │
                 └────────────────────────────────────────────┘ │
                                                                 ▼
                          ┌───────────────────────────────────────────┐
                          │  Core-owner thread (owns !Send HostCore)    │
                          │   - exclusive flock on log.bin (held)       │
                          │   - serial apply of CoreCmd → reply         │
                          └───────────────────────────────────────────┘
```

- `CoreCmd` is a small enum (`Invoke`, `AppActions`, `PermissionCheck`,
  `Approve`, `Deny`, `AdminView`, …), each carrying a `oneshot`-style reply
  channel. Front-ends never touch the Core directly; they build a `CoreCmd`, send
  it, and block on the reply. This is the only place the Core is mutated, so the
  actor is the in-process single writer.
- The stdio front-end stays synchronous request/response from the model's view;
  it just talks to the actor instead of a local `&mut Core`.
- **Elicitation exception (see below):** the elicitation round-trip needs the
  *stdio front-end itself* to emit a frame and read a reply, so the elicitation
  wait lives in the stdio front-end, not the actor. The actor only performs the
  final `Approve` once the human's answer is in hand. The actor never blocks on
  I/O — it stays responsive to the admin front-end throughout.

### Threading note

The actor owns `HostCore` and never leaves its thread, satisfying `!Send`.
Front-end threads hold only channel endpoints (`Send`). No `Mutex<Core>`, no
`unsafe`. The loopback listener is a plain blocking `TcpListener` on
`127.0.0.1` in its own thread.

## Problem 1 — Transport

### 1a. Elicitation (primary, in-session)

Flow, model side unchanged except it may now succeed on the first call:

1. Model calls `invoke` → actor computes `permission_required` (the request is
   recorded `pending` as today).
2. If the client advertised the `elicitation` capability at `initialize`, the
   **stdio front-end** sends `elicitation/create` on stdout with a fresh JSON-RPC
   **request id distinct from the tool-call id**:
   - `message`: human-readable ("App *Focus Hub* requests `kv`, `crdt`. Grant to
     `user:local-owner`?").
   - `requestedSchema`: a minimal object schema with an enum `decision`
     (`approve`/`deny`) so form-mode clients render buttons; URL-mode clients get
     the `adminUrl` in the message as a fallback.
3. The front-end runs a **nested receive** on stdin: read frames until the one
   whose `id` matches our elicitation id. The client is blocked awaiting our
   tool result, so nothing else should arrive; still, handle interleaving
   defensively — a `notifications/cancelled` for the original tool call aborts
   the wait; unrelated frames are parked/deferred (or answered if trivially safe
   like `ping`). A wall-clock **timeout** (`TERRANE_ELICIT_TIMEOUT_MS`, default
   e.g. 120000) bounds the wait.
4. On `approve` → front-end sends `CoreCmd::Approve{request_id}` to the actor →
   actor calls `approve_permission_request` (trusted, in-process) → grant folds
   into the live Core → front-end **retries the invoke** (or returns success) and
   the tool result is the app output. No restart.
5. On `deny`/decline/cancel → deny the request, return a clean denied result.
6. On timeout **or** client without elicitation capability → return today's
   `permission_required` payload unchanged (the documented poll flow still works).

Engine change: `handle_json_rpc` (`terrane-host/src/mcp.rs:29`) currently is a
pure function of `(core, raw) → Option<String>`. The elicitation wait needs the
front-end to *write a request and read a reply mid-handling*. So the stdio
front-end gets a small transport object (writer + line reader) that the invoke
handler can call to elicit. Parse and store the client's declared capabilities
from `initialize` (today ignored — `mcp.rs:78`) and only elicit when
`elicitation` is present.

### 1b. Admin control-plane (secondary, console / headless)

Host the **existing** admin approve/deny logic (`host/web/src/admin.rs:269–480`)
inside `terrane-mcp` as a loopback listener routing to `CoreCmd::Approve/Deny`
against the same actor Core. Because it targets the *live* Core, a console
approval is seen by the model's next `permission_check`/retry with no restart —
same guarantee as elicitation, available to any client. Bind `127.0.0.1` only;
document that this is a **trusted-operator** surface (loopback = same-machine
human). Reuse `admin_authorized` (`host/web/src/http.rs:22`) semantics.

## Problem 2 — Concurrency & durability

`Core::append` holds **no lock** and writes each record as two `write_all`s
(`terrane-core/src/lib.rs:987`). Two processes on one `$TERRANE_HOME` diverge
silently and can tear the log. Fixes:

- **Exclusive advisory lock on `log.bin`** taken in `Core::open_with` (or the
  host `open_at_log_path`) and held for process life via a stored lock guard. A
  second writer fails fast: *"another terrane process holds $TERRANE_HOME;
  approve in the running session or stop it first."* Add a small dependency
  (`fs2`/`fs4`) or use raw `flock`/`LockFileEx` behind a tiny cross-platform
  shim. **This is the mechanism that enforces single-source-of-truth** — it is
  *why* the grant becomes visible live.
- **Single-frame atomic append** — build `len ++ payload` in one buffer, one
  `write_all` — defense-in-depth against torn records.
- **Read path (follow-up):** allow read-only CLI opens to take a **shared** lock
  so `terrane list`-style commands coexist with a running server; writers still
  require exclusive.

### Intended behavior change (confirmed)

`terrane auth grant …` in a second terminal **while the session's server runs**
now **fails** (exclusive lock). Approval goes through the live session
(elicitation) or the in-process admin console. Document this in
`AGENT_PLAYBOOK.md`, `SECURITY.md`, and `docs/model-call-mcp.md`.

## Security analysis (invariant that must hold)

Approval is a **human action in the client/console UI, carried over MCP's
server→client back-channel (or the loopback admin socket)** — never a tool the
model can call. Every existing barrier is preserved:

- No grant/approve tool in the model's tool surface.
- `capability_command` refuses `auth.*` and dispatches Public (`mcp.rs:734,747`).
- Core gate rejects Public `auth.*` (`core/src/lib.rs:1012`).
- The actor performs the grant with trusted authority **only** in response to an
  `Approve` `CoreCmd`, which is producible **only** by the elicitation-approve
  path or the loopback admin front-end — neither reachable by the model.

Elicitation adds a *human*, not a model capability. The loopback listener is
same-machine/trusted-operator by construction. No new way for the model to
escalate exists.

## Failure modes & policy

| Situation | Behavior |
|---|---|
| Client lacks `elicitation` cap | Return `permission_required` (unchanged flow) |
| Human approves | Grant folds live; retry succeeds; tool returns app output |
| Human denies / declines | Deny request; return denied result with reason |
| Elicitation timeout | Return `permission_required`; request stays `pending` |
| Client cancels tool call mid-elicit | Abort wait; leave request `pending` |
| Second writer process | Fail fast with lock message |
| Console approve while model idle | Seen on model's next `permission_check`/retry |
| Approve an already-approved request | Idempotent (`approve` no-ops, `lib.rs:506`) |

## Phased delivery (each phase ends green: `cargo test` + `cargo clippy -D warnings`)

**Phase 1 — Durability foundation** *(independently valuable, unblocks all)*
- Exclusive lock in the core open path; store guard on `Core`; release on drop.
- Single-frame atomic append.
- Tests (own files): two-process contention fails fast; lock released on exit;
  torn-write cannot occur; replay-identity preserved.

**Phase 2 — Core-owning actor**
- `CoreCmd` enum + owner thread + channel; stdio front-end rewired to submit
  commands. No behavior change yet (pure refactor); existing MCP tests stay green.
- Tests: actor serializes concurrent submissions; stdio parity with pre-refactor.

**Phase 3 — Elicitation approve (in-session)**
- Parse client capabilities at `initialize`; capability-gated elicitation.
- Nested-receive transport in the stdio front-end; invoke handler elicits on
  `permission_required`; approve→retry.
- Tests: scripted stdio client covering approve / deny / timeout /
  no-elicitation-fallback / tool-cancel. e2e through the real `terrane-mcp`
  binary. Live opencode run proving no restart.

**Phase 4 — Admin control-plane on the live Core**
- Loopback listener in `terrane-mcp` reusing `admin.rs` approve/deny → `CoreCmd`.
- Tests: console approve seen by live model session; loopback-only bind; deny
  path; audit reflects decided-by/reason.

**Docs pass (with phases 3–4):** update `AGENT_PLAYBOOK.md`, `SECURITY.md`,
`APP_BUILDING.md`, `docs/model-call-mcp.md`, and `terrane-api` tool descriptions
for the in-session approve flow and the single-writer lock behavior.

## Acceptance (canonical)

1. Model `invoke` on a denied resource → operator sees an in-session prompt →
   approves → the **same** `invoke` returns the app output, **no restart**.
2. Same, approved from the loopback admin console instead of elicitation.
3. Model cannot produce an `Approve` by any tool call (grant tool absent;
   `capability_command auth.*` refused; Public `auth.*` gated).
4. A second `terrane auth grant` process fails fast while the server holds the
   lock; replaying the log reproduces identical state (replay-identity holds).
5. Client without elicitation still gets `permission_required` and the existing
   poll flow.

## Tests live in their own files (CLAUDE.md)

Per-capability engine tests under `terrane-core/tests/cap/`; host/broker tests
under `terrane-host/tests/`; the binary-level e2e under
`terrane-host/tests/cap/` and the `terrane-mcp` crate's own tests. Effectful e2e
(real opencode elicitation round-trip) is `#[ignore]`d with a reason; the
default `cargo test` stays green.
