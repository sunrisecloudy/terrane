# Capability: `common` — `common.send`, outbound messaging by channel

New crate `rust/crates/terrane-cap-common/`, namespace `common`, registered in
`default_registry`. The platform half of the common API: apps **implement**
`common.receive` ([cap-interop.md](cap-interop.md)); apps **call**
`common.send` to deliver a message to an external destination. Full symmetry —
one verb in, one verb out.

**Email is not a capability; it is the first channel.** `common.send` takes a
`channel` and a channel-shaped message; the edge delivers it once and the
outcome is recorded. SMS, chat webhooks, or any future transport are new
channels behind the same command and the same event — never new caps.

## Locked decisions (user, 2026-07-05)

1. **Renamed from `email` to `common`** with a channel model; `common.send`
   mirrors `common.receive`.
2. **Grants are channel-scoped**, never blanket: the selector is
   `common:send:<channel>` so the permission prompt is legible — "Allow Todo
   to send **email**?", not "allow common".
3. Inbound (v2) stays an interop delivery: intake at the edge →
   `common.receive("email", …)` to the user-routed app. No new app surface.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `common.send` | args `app, message_json` → validate purely per channel schema, compute `body_hash` in decide, return `Decision::Effect(Effect::ChannelSend {app, channel, message})` — **recorded** |
| Event | `common.sent` | `{app, channel, message_id, to, subject?, body_hash, body_kind, body, attachments: [{name, hash, size, mime}], status, error, sent_at}` — channel-agnostic envelope, channel-specific fields optional |
| Resource | `common.send(messageJson)` | routes to the command (recorded — outward-facing effects are always auditable, no transient variant) |
| Resource | `common.status(messageId)` | pure state read |
| Query | `common.channels` | channels configured on this host + which the calling app is granted |
| (reacts) | `app.removed` | drop the app's sent map |

`message_json` (email channel):

```jsonc
{
  "channel": "email",                 // required; selects schema + grant + transport
  "to": ["a@example.com"],            // 1–20 recipients total across to/cc/bcc
  "cc": [], "bcc": [],
  "subject": "Weekly report",
  "text": "…",                        // required
  "html": "…",                        // optional alternative part
  "attachments": ["reports/w27.pdf"], // app blob names (cap-blob.md), ≤ 10, ≤ 20 MiB total
  "recordBody": false,                // opt-in full-body recording
  "connection": "smtp-default"        // optional; channel's default transport otherwise
}
```

Fold keeps `app → message_id → SentMeta {channel, to, subject?, body_hash,
status}`. `message_id` is minted by the edge (email: RFC 5322 Message-ID) and
returned in the event; `status` is `"sent" | "failed"` with `error` text — a
failed send is still a fact and still folds.

## Channels

A channel = a name + a message schema (validated in decide) + an edge
transport (resolved through a named connection,
[cap-oauth-connections.md](cap-oauth-connections.md)) + a grant selector.

| Channel | Status | Transport |
| --- | --- | --- |
| `email` | v1 | SMTP via connection kind `smtp` (default name `smtp-default`); provider adapters (Resend/SES/Gmail API over [net-v2](cap-net-v2.md) + oauth2) later, behind the same event |
| `sms`, `chat-webhook` (Slack/Discord/Matrix) | future | new schema + transport, same `common.send`/`common.sent` |

Unknown channel → typed error listing configured channels (agents
self-correct). Channel schemas live in the crate; adding one is additive.

## What gets recorded (privacy stance)

**Recipients + subject + `sha256(body)` are recorded; the full body is not,
unless the app opts in.** Message bodies are often the most personal data an
app touches; the log is plaintext. So:

- `body_hash` always (integrity/audit anchor, same role as net-v2 `body_hash`).
- `recordBody: true` inlines the body up to 256 KiB, else blob-offloads per
  the [cap-blob.md](cap-blob.md)/net-v2 convention (`body_kind: "blob"`).
  Default `false` ⇒ `body_kind: "none"`.
- Attachments are **always blob refs** — bytes live in the CAS, never in the
  event.
- `bcc` is recorded (the sender's own log may know its own secrets about
  recipients) but `describe()` and MCP event dumps print only channel + `to`
  count + subject — no addresses. Aligns with net-v2's describe philosophy.

## Transport (edge)

- Connection resolution at the edge from the secret store — **credentials
  never appear in events** (the event records the connection *name* only),
  exactly the net-v2 `{"$secret"}` contract.
- No automatic retries (net-v2 stance). The app sees `status: "failed"` and
  decides.
- Email: edge builds MIME (text + optional html + attachments read from the
  CAS by hash) via `lettre` (decision below).

## Security & permissions

- Grant selector `common:send:email` with prompt wording that says what it
  is: **"Send email to real recipients as you. Messages leave this machine
  and cannot be recalled."** Outward-facing grants are explicit — never in a
  scaffold's default resource list. Each future channel writes its own prompt.
- Per-app, per-channel rate limits enforced in decide against folded state:
  email **20 sends/hour, 100/day** (typed error naming the limit).
- Recipient validation: syntactic (RFC 5321 addr-spec) in decide; no MX
  lookups (an effect inside decide is forbidden).
- Two gates: "may send on this channel" and "may use this connection"
  ([cap-oauth-connections.md](cap-oauth-connections.md) per-app grants).

## Limits (documented in `doc.rs`)

Email: ≤ 20 recipients/message; subject ≤ 998 chars; body ≤ 1 MiB text,
2 MiB html; ≤ 10 attachments, ≤ 20 MiB total; 20/hour, 100/day per app.
Channel schemas carry their own limits.

## Implementation plan

1. **Interface:** add `Effect::ChannelSend { app, channel, message: String }`
   (canonical JSON, secrets never inside) to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-common`:** channel schema registry (email first),
   parse/validate/canonicalize, `body_hash` in decide, per-channel rate limits
   against folded state, fold, `sent_event()` constructor, describe (channel +
   counts, no addresses), `doc.rs`.
3. **Edge:** `ChannelSend` arm in `EdgeRunner::run`
   (`terrane-host/src/edge.rs`): connection resolution via the secret store,
   email = MIME assembly from CAS attachments + SMTP submit, return
   `[common.sent]`. Depends on
   [cap-oauth-connections.md](cap-oauth-connections.md) step 3 (resolver).
4. **App surface:** `APP_API.md` — `ctx.resource.common.send/status/channels`,
   manifest `resources: ["common"]` with channel grants; scaffold recipe
   deliberately does **not** include it by default.
5. **Tests:** engine (`terrane-core/tests/cap/common.rs`): channel validation,
   rate limits, recordBody variants, fold/replay identity, app.removed,
   unknown-channel error. E2e (`terrane-host/tests/cap/common.rs`): loopback
   SMTP test server (bind `127.0.0.1:0`) — send with attachment from CAS,
   failure fold; default-run. Real-provider send stays `#[ignore]`.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## v2 (planned, user-confirmed 2026-07-05): receiving

Inbound is a delivery problem, not a channel problem: a received message is a
payload looking for an app. It rides [cap-interop.md](cap-interop.md) — the
host's intake (email: IMAP/JMAP poll or a provider inbound webhook via
[cap-webhook.md](cap-webhook.md)) records
`common.received {channel, message_id, from, to, subject?, body_ref}` and
delivers to the user-routed app through the required
`common.receive(channel, …)` verb, exactly like any other interop sender.
Routing = address-per-app (`<app>@<user-domain>`) or user rules via the
interop picker.

## Non-goals (v1)

Receiving (v2 above), templating, scheduling/queued retry (that's
[cap-job-queue.md](cap-job-queue.md) calling `common.send`), bounce tracking,
provider-specific APIs, HTML sanitization (the app authors its own body),
non-email channels (schema slots exist; transports are future work).

## Decisions to confirm

- **Body recording default = hash-only, opt-in full body** — *recommend: as
  specced* (privacy by default; the hash still anchors audit) —
  *alternatives:* always record (simplest replay story, worst privacy);
  never record (loses the debugging/audit value entirely).
- **SMTP-first transport via a named connection** — *recommend: as specced*
  (one credential story shared with every cap) — *alternative:* provider HTTP
  APIs first (better deliverability, but N adapters before the first email
  sends).
- **`lettre` as the MIME/SMTP crate** — *recommend: yes* (mature, rustls,
  no C deps) — *alternative:* hand-rolled MIME over a TCP client; smaller
  tree, large correctness surface.
- **Failed sends still fold as `common.sent {status: "failed"}`** —
  *recommend: yes* (rate limits must count attempts; the outcome is a fact) —
  *alternative:* a separate `common.send-failed` kind; two kinds, same fold.
