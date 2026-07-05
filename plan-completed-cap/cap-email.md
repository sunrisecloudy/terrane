# Capability: `email` — outbound email as a recorded effect

New crate `rust/crates/terrane-cap-email/`, namespace `email`, registered in
`default_registry`. Lets an app send email — reports, alerts, invitations.
**Receiving email is a non-goal in v1** (see non-goals).

Sending is an effect exactly like a [net-v2](cap-net-v2.md) request: the core
decides, the edge sends once, and the outcome is recorded as an event. Replay
folds `email.sent` and **never re-sends** — the same replay story as every
other effect, and the reason retries are not automatic.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `email.send` | args `app, message_json` → validate purely, compute `body_hash` in decide (blob-put pattern), return `Decision::Effect(Effect::EmailSend)` — **recorded** |
| Event | `email.sent` | `{app, message_id, to, cc, subject, body_hash, body_kind, body, attachments: [{name, hash, size, mime}], status, error, sent_at}` |
| Resource | `email.send(messageJson)` | routes to the command (recorded — outward-facing effects are always auditable, no transient variant) |
| Resource | `email.status(messageId)` | pure state read |
| (reacts) | `app.removed` | drop the app's sent map |

`message_json`:

```jsonc
{
  "to": ["a@example.com"],            // 1–20 recipients total across to/cc/bcc
  "cc": [], "bcc": [],
  "subject": "Weekly report",
  "text": "…",                        // required
  "html": "…",                        // optional alternative part
  "attachments": ["reports/w27.pdf"], // app blob names (cap-blob.md), ≤ 10, ≤ 20 MiB total
  "recordBody": false,                // opt-in full-body recording
  "connection": "smtp-default"        // optional; default transport otherwise
}
```

Fold keeps `app → message_id → SentMeta {to, subject, body_hash, status}`.
`message_id` is minted by the edge (RFC 5322 Message-ID) and returned in the
event; `status` is `"sent" | "failed"` with `error` text on failure — a failed
send is still a fact and still folds.

## What gets recorded (privacy stance)

**Recipients + subject + `sha256(body)` are recorded; the full body is not,
unless the app opts in.** Email bodies are often the most personal data an app
touches; the log is plaintext. So:

- `body_hash` always (integrity/audit anchor, same role as net-v2 `body_hash`).
- `recordBody: true` inlines the body up to 256 KiB, else blob-offloads per
  the [cap-blob.md](cap-blob.md)/net-v2 convention (`body_kind: "blob"`).
  Default `false` ⇒ `body_kind: "none"`.
- Attachments are **always blob refs** (`{name, hash, size, mime}`) — bytes
  already live in the CAS, never in the event.
- `bcc` is recorded (the sender's own log may know its own secrets about
  recipients) but `describe()` and MCP event dumps print only `to` count +
  subject — no addresses. Aligns with net-v2's describe philosophy.

## Transport (edge)

- **Host-level SMTP config** is the v1 transport: a named connection of kind
  `smtp` in [cap-oauth-connections.md](cap-oauth-connections.md) —
  `{host, port, starttls, username, password: {"$secret": "…"}}`. The default
  connection name is `smtp-default`; `message_json.connection` selects
  another. Provider adapters (Resend/SES/Gmail API over
  [net-v2](cap-net-v2.md) + oauth2) are later additions behind the same event.
- **Credentials never appear in events** — the event records the connection
  *name* only; resolution happens at the edge from the secret store, exactly
  the net-v2 `{"$secret"}` contract.
- No automatic retries (net-v2 stance: a retry re-runs an effect the log has
  an opinion about). The app sees `status: "failed"` and decides.
- Edge builds MIME (text + optional html alternative + attachments read from
  the CAS by hash) via a vetted MIME/SMTP crate (`lettre` is the default
  candidate — decision below).

## Security & permissions

- Grant resource `email` (namespace-v1) with prompt wording that says what it
  is: **"Send email to real recipients as you. Messages leave this machine
  and cannot be recalled."** Outward-facing grants must be explicit — never
  bundled into a scaffold's default resource list.
- Per-app rate limits enforced in decide against folded state: **20
  sends/hour, 100/day** (typed error naming the limit). A runaway backend
  cannot spam.
- Recipient validation: syntactic (RFC 5321 addr-spec) in decide; no MX
  lookups (that would be an effect inside decide).
- The app needs a grant on the named connection too
  ([cap-oauth-connections.md](cap-oauth-connections.md) per-app grants) — two
  gates: "may send email" and "may use this identity".

## Limits (documented in `doc.rs`)

- ≤ 20 recipients/message; subject ≤ 998 chars; body ≤ 1 MiB text, 2 MiB html.
- ≤ 10 attachments, ≤ 20 MiB total (read from CAS, so no base64 transit).
- Rate: 20/hour, 100/day per app.

## Implementation plan

1. **Interface:** add `Effect::EmailSend { app, message: String }` (canonical
   JSON, secrets never inside) to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-email`:** message parse/validate/canonicalize,
   `body_hash` in decide, rate-limit check against folded state, fold,
   `sent_event()` constructor, describe (counts, no addresses), `doc.rs`.
3. **Edge:** `EmailSend` arm in `EdgeRunner::run`
   (`terrane-host/src/edge.rs`): resolve connection via the secret store, MIME
   assembly from CAS attachments, SMTP submit, return `[email.sent]`. Depends
   on [cap-oauth-connections.md](cap-oauth-connections.md) step 3 (resolver).
4. **App surface:** `APP_API.md` — `ctx.resource.email.send/status`, manifest
   `resources: ["email"]`; scaffold recipe deliberately does **not** include
   it by default.
5. **Tests:** engine (`terrane-core/tests/cap/email.rs`): validation, rate
   limits, recordBody variants, fold/replay identity, app.removed. E2e
   (`terrane-host/tests/cap/email.rs`): loopback SMTP test server (bind
   `127.0.0.1:0`) — send with attachment from CAS, failure fold; default-run.
   A real-provider send stays `#[ignore]` (reason: external effect), matching
   the net/model convention.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Receiving email (IMAP/JMAP or provider inbound webhooks — the latter would be
a [cap-webhook.md](cap-webhook.md) consumer, not new machinery), templating,
scheduling/queued retry, bounce tracking, provider-specific APIs, HTML
sanitization (the app authors its own body).

## Decisions to confirm

- **Body recording default = hash-only, opt-in full body** — *recommend: as
  specced* (privacy by default; the hash still anchors audit) —
  *alternatives:* always record (simplest replay story, worst privacy);
  never record (loses the debugging/audit value entirely).
- **SMTP-first transport via a named connection** — *recommend: as specced*
  (one credential story shared with every cap via
  [cap-oauth-connections.md](cap-oauth-connections.md)) — *alternative:*
  provider HTTP APIs first (better deliverability, but N adapters before the
  first email sends).
- **`lettre` as the MIME/SMTP crate** — *recommend: yes* (mature, rustls,
  no C deps) — *alternative:* hand-rolled MIME over a TCP client; smaller
  tree, large correctness surface.
- **Failed sends still fold as `email.sent {status: "failed"}`** —
  *recommend: yes* (rate limits must count attempts; the outcome is a fact) —
  *alternative:* a separate `email.failed` kind; two kinds, same fold.
