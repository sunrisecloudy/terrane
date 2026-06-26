# 14 — Effect Surface & Observability (the two-door decision)

This file records a design decision that shapes the whole initiative: **should
every native action a JS applet performs (data access, API calls, file
read/write, UI) pass through the same door as operator commands, so everything is
observable, testable, and agent-drivable?**

**Decision (locked for v1):** Yes to the *goal* — one observable, self-describing,
replayable effect surface. **No** to the literal version (routing every `ctx.*`
host-call back out through `WorkspaceCore::handle` as a `CoreCommand`). Instead:
**two execution doors that share one catalog, one journal, one policy model, and
one test/replay harness.**

## Why the goal is already ~70% real

The inner surface is **already a single mediated, recorded seam** — this is not
new work, it is a consequence of the replay requirement.

- **One chokepoint:** the `HostBridge` trait — *"the seam through which the
  sandbox performs effects… injected, capability-checked, and recordable"*
  (`forge/crates/runtime/src/bridge.rs:1`). Every `ctx.*` call is one method:
  `storage_get/set/delete/list`, `db_insert/get/update/patch/delete/transact/
  list/query/watch`, `ui_render`, `net_fetch`, `files_write`, `secret_store`,
  `log` (`bridge.rs:33`–`196`).
- **One journal:** the recorder appends a `RecordedCall { seq, method, args,
  response }` for every host interaction, producing a `RunRecord`
  (`forge/crates/domain/src/run.rs:49` and `:65`). Replay *serves* the recording
  and fails on any divergence (`forge/crates/runtime/src/recorder.rs`).
- **One policy model:** each method is capability-checked per applet before the
  effect runs.

So **observe** and **test** — two of the three goals — are *already delivered*
for the inner surface. The platform had to build this to make deterministic
replay (prd-merged/01 CR-8/CR-11) work at all. We are standing on it, not
starting from zero.

## Why NOT collapse into one literal entrypoint

Routing every `ctx.db.get` / `files.read` / `net.fetch` back out through the
outer `handle(CoreCommand)` door would be a category error:

1. **Re-entrancy.** Host-calls happen *inside* a `runtime.run` that the outer
   door already invoked. The run holds the core; nesting calls back through the
   same entrypoint is awkward and serializes needlessly.
2. **Granularity / performance.** Host-calls are high-frequency (render loops,
   many reads). The outer pipeline (RBAC match → envelope alloc → JSON
   round-trip) is built for coarse, occasional operator commands.
3. **Trust mismatch (the decisive one).** Outer commands are authorized by **user
   role** (Owner/Editor — `forge/crates/core/src/auth.rs:28`). Inner host-calls
   are authorized by **applet capability grants** (this applet may fetch this
   domain). An applet is *not* a role-actor. Flattening the two auth models is a
   security hazard.
4. **Transactionality.** The outer command *is* the transaction boundary; host-
   calls participate inside it. Re-issuing them as top-level commands breaks that
   nesting.

## The two doors

```
   OUTER DOOR — operator / agent surface
   WorkspaceCore::handle(CoreCommand)          authz: user ROLE (auth.rs)
   workspace.* applet.* runtime.* query.*      coarse, transactional, occasional
   ui.dispatch_event  sync.* quota.* …         ← the CLI / console / agent drive this

   INNER DOOR — applet sandbox surface
   HostBridge methods  (ctx.*)                 authz: applet CAPABILITY grants
   db.* storage.* files.* net.fetch ui.render  fine-grained, in-run, high-frequency
   recorded as RecordedCall → RunRecord        ← the running app drives this
```

**Shared across both doors (this is the real "one door"):**

| Shared layer | Mechanism | Status |
| --- | --- | --- |
| One **catalog** | `CommandDescriptor` with `surface: inner\|outer` (file `04`) | plan |
| One **journal** | `RecordedCall` / `RunRecord` (`run.rs:49`) | **exists** |
| One **policy/capability** engine | `forge-policy` + bridge checks | exists |
| One **replay/test** harness | recorder record/replay (`recorder.rs`) | exists |
| One **observability** read | `system.trace` (new, below) | plan |

## New: `system.trace` (observability through the outer door)

The journal exists but is not yet queryable by an operator/agent. Add a read-only
outer command that surfaces it:

```jsonc
// request
{ "name": "system.trace",
  "payload": { "run_id": "…", "since_seq": 0, "methods": ["net.fetch"] } }

// response
{ "ok": true,
  "payload": { "run_id": "…",
    "calls": [ { "seq": 0, "method": "db.insert", "args": {…}, "response": {…} } ],
    "truncated": false } }
```

- **Pure read** of the recorded `RunRecord` — no effects, replay-safe, like
  `system.describe` (file `06`).
- Lets the CLI (`forge trace <run>`), the console, and an agent **observe every
  inner effect a run produced** — without being able to *forge* one.
- Visibility `operator`; redaction follows the same audit rules as `audit.query`
  (secrets stay redacted).
- This is the bridge between the two doors: the inner door *produces* the trace;
  the outer door *exposes* it.

## "Let an agent do anything the UI can" — via `ui.dispatch_event`

The cleanest realization does **not** hand the agent raw host-calls. Note the
asymmetry already in the system:

- the app **renders** a UI tree → inner host-call `ui_render` (recorded);
- user input **comes in** → the **outer** command `ui.dispatch_event` (already in
  the registry, `commands/mod.rs:104`).

So an agent that can call **`ui.dispatch_event` drives the app exactly like a
human clicking**, and every host-call the app makes in response is recorded
automatically. You get **agent = UI through the outer door**, while the inner
door stays the app's sandbox. The agent never forges a `net.fetch`; it dispatches
the event a button would and the app does the rest. This is principled
computer-use: the agent's reach equals the UI's reach *by construction*.

The agent adapter (file `09`) therefore offers, by default: the public/operator
outer commands **plus** `ui.dispatch_event` for the mounted app — and `ctx.*`
entries are **describe-only** (see below), never callable tools.

## Catalog treatment of the inner surface

- Inner host-calls appear in the catalog with `surface: "inner"` (file `04`).
- `forge describe ctx.net` / `describe db.insert` **works** — full documentation
  and schema (file `13` Q6).
- `forge run <inner>` is **refused** with a pointer to the app runtime (file
  `07`); the agent adapter never emits them as tools (file `09`).
- This gives agents/operators a *complete reference* of what the sandbox can do,
  without letting them issue host-calls directly.

## What this adds to the plan (small)

- **Phase 1** (`05`): the catalog already carries `surface`; just ensure inner
  host-calls (the `HostBridge` methods) are enumerated as `surface: inner`
  descriptors. No new auth.
- **Phase 2** (`06`): add `system.trace` next to `system.describe` — same pure-read
  pattern.
- **Phase 3** (`07`): `forge trace <run_id>` reads `system.trace`.
- **Phase 4/5** (`08`/`09`): console shows a per-run effect timeline; agent gets
  `ui.dispatch_event` + observe-only trace; no inner tools.

No handler bodies change; no new effect path; determinism untouched.

## Validation / exit criteria

- `system.trace` returns the recorded `RunRecord` for a run, filtered by
  `methods`/`since_seq`, with secrets redacted.
- A trace is **read-only and replay-safe** (no events, no mutation).
- The catalog lists inner host-calls as `surface: inner`; `run`/agent refuse them;
  `describe` documents them.
- An agent driving only `ui.dispatch_event` can reproduce a UI interaction, and
  `system.trace` shows the resulting inner effects.

## Decision summary

> **Two doors, one mediator's worth of guarantees.** Keep the operator/command
> door and the applet/host-call door separate at execution (different trust,
> granularity, transaction scope), but unify them at the **catalog**, **journal**,
> **policy**, and **observability** layers. Achieve "agent does anything the UI
> can" through `ui.dispatch_event`, and make every inner effect observable through
> `system.trace`. This delivers the original intent — observe, test, agent-drive —
> without the re-entrancy, performance, and trust costs of a single literal
> entrypoint.

See also: [02-PLATFORM-OVERVIEW.md](02-PLATFORM-OVERVIEW.md) (two-surface model),
[04-COMMAND-CATALOG.md](04-COMMAND-CATALOG.md) (`surface` field, inner entries),
[06-PHASE-2-INTROSPECTION-COMMAND.md](06-PHASE-2-INTROSPECTION-COMMAND.md)
(`system.describe`/`system.trace`), [09-PHASE-5-AGENT-ADAPTER.md](09-PHASE-5-AGENT-ADAPTER.md)
(agent surface), [13-OPEN-QUESTIONS.md](13-OPEN-QUESTIONS.md) Q6.
