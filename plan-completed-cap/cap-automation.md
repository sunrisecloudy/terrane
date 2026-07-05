# Capability: `automation` — event-triggered rules

New crate `rust/crates/terrane-cap-automation/`, namespace `automation`. The
missing trigger type: [cap-scheduler.md](cap-scheduler.md) fires on **time**;
automation fires on **events** — "when X happens, run verb Y". IFTTT-style
rules that users set through the shell and agents set on request ("notify me
when an email arrives", "when a todo is added, file it"). Cloudflare shipped
the same shape as Queues event-subscriptions; for a personal event-sourced
platform it is even more natural: the events already exist.

## Design

A rule is a recorded fact; firings are recorded facts; the matcher runs at the
host edge over freshly committed events — replay folds rules and firings but
**never re-fires** (identical to `scheduler.fire`'s replay stance).

```jsonc
// automation.set {app, name, rule_json}
{
  "trigger": {
    "kind": "kv.set",                    // event kind or prefix ("kv.*")
    "sourceApp": "mailbox",              // defaults to own app; other apps need a grant
    "filter": "key starts_with 'inbox/'" // optional JMESPath over the event payload
  },
  "action": {
    "verb": "summarize",                 // verb on the rule-owning app
    "argsTemplate": ["{{event.key}}"]    // payload fields spliced as args
  },
  "cooldownMs": 1000                     // per-rule floor between firings
}
```

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `automation.set` | validate (kind exists in registry manifests, JMESPath parses, verb non-internal), recorded `automation.set {app, name, rule, rule_hash}` |
| Command | `automation.rm` | recorded `automation.removed` |
| Command | `automation.fire` | **TrustedHost only** (the authority already exists — scheduler uses it): `{app, name, rule_hash, event_ref, fired_at}` recorded; the host then dispatches the action verb via the normal `js-runtime.run` path (the scheduler.fire pattern exactly) |
| Query | `automation.list` / `automation.stat` | folded rules + last-fired/fire-count |
| Resource | `automation.set/rm/list` | apps manage their own rules (an agent building "auto-file" features uses this) |
| (reacts) | `app.removed` | drop the app's rules |

## Loop prevention & safety

- **Causality fence:** the host tags dispatches it makes on behalf of a rule;
  events produced by rule-driven runs carry that provenance in the dispatch
  context, and the matcher skips events caused by the same rule's own chain
  (depth 1 by default). A→B→A cross-rule cycles are cut by a global
  **per-commit fire budget** (≤ 8 rule firings triggered by one original
  command) — beyond it, firings are skipped and recorded as
  `automation.suppressed` so the loop is visible, not silent.
- Cooldown floor 1 s per rule (configurable upward), ≤ 32 rules per app.
- Cross-app triggers (`sourceApp` ≠ own app) require an explicit grant via the
  standard elicitation — observing another app's events is read access to its
  data. Payloads cross through the same filter only; the action still runs
  with the rule-owner's scope.

## Replay story

`automation.set/removed/fire/suppressed` are ordinary folded facts. The
matcher is edge machinery (runs post-commit on the live host, like the
scheduler daemon; CLI host = `terrane automation tick` parity). Replay
rebuilds the rule table and the firing history; it never evaluates a matcher.

## Relations

- [cap-push.md](cap-push.md)'s `push.subscribe {event_pattern, template}` is
  an automation rule whose action is "notify" — when both land, push should
  ride this matcher rather than keep its own (noted there as convergence).
- Filters reuse the JMESPath engine from [cap-query.md](cap-query.md) (one
  expression dialect platform-wide).
- Time triggers stay in [cap-scheduler.md](cap-scheduler.md); an automation
  action MAY set/remove schedules — the two compose, they don't merge.

## Implementation plan

1. **Crate:** rule parse/validate (JMESPath via the query cap's wrapper),
   decide (incl. TrustedHost fire), fold (rules + firing stats), doc, describe.
2. **Edge matcher:** post-commit hook in the long-running hosts (web/mac):
   match committed events against folded rules → dispatch `automation.fire` +
   the action verb; provenance tagging + fire budget; `terrane automation
   tick` for the CLI.
3. **Shell:** rules panel (list/enable/disable) + the cross-app-trigger grant
   prompt.
4. **`APP_API.md`:** `ctx.resource.automation.*` with the mailbox→summarize
   worked example.
5. **Tests:** engine (validation, fold, replay-never-fires); e2e (rule fires
   on a real kv.set, cooldown honored, loop suppressed and recorded,
   cross-app grant enforced).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Multi-step rule chains (compose via [cap-job-queue.md](cap-job-queue.md)),
scheduled+event combined conditions, rule marketplaces, cross-replica rules
(rides sync later).

## Decisions to confirm

- **JMESPath as the filter dialect** — recommend yes (already in the platform
  via query) — alternative: simple field-equality matchers only (weaker, no
  new failure modes).
- **Per-commit fire budget = 8, chain depth = 1** — recommend as specced —
  alternative: deeper chains allowed with explicit `allowChain: true` per rule.
- **Cross-app trigger grant granularity** — recommend per (observer app →
  source app) — alternative: per (observer → source → event-kind), finer but
  noisier prompts.
