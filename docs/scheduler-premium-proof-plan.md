# Scheduler Premium Ops Proof Plan

Public Terrane owns the scheduler capability and host runner. Premium should
consume the public contract after this lands; it should not rely on hidden
Premium-only runtime semantics.

## Public contract dependency

Premium should refresh its pinned Terrane contract and verify that the contract
contains:

- capability namespace `scheduler`
- `ctx.resource.scheduler.set(name, specJson)`
- `ctx.resource.scheduler.clear(name)`
- `ctx.resource.scheduler.list()`
- `ctx.resource.scheduler.stat(name)`

The grant required for the ops app is the public `scheduler` namespace grant.

## Premium ops app behavior

The Premium-owned ops/admin app should declare `scheduler` in its app manifest
resources and schedule the proof action from its backend:

```js
await ctx.resource.scheduler.set(
  "quickjs-ops-heartbeat",
  JSON.stringify({
    cron: "* * * * *",
    verb: "opsHeartbeat",
    args: ["premium-ops-proof"]
  })
);
```

The host records `scheduler.fired`, then invokes the action as
`handle([verb, name, scheduledFor, ...args])`:

```js
function handle(input) {
  if (input[0] === "opsHeartbeat") {
      return JSON.stringify({
        ok: true,
        runtime: "quickjs",
        name: input[1],
        scheduledFor: input[2],
        source: input[3]
      });
  }
  return "unknown";
}
```

## Ops-admin display

The Premium ops-admin surface should show:

- QuickJS runtime activity from normal app invocation telemetry or run output.
- Scheduler definitions from `ctx.resource.scheduler.list()`.
- Last fire metadata from `ctx.resource.scheduler.stat("quickjs-ops-heartbeat")`.
- `last_scheduled_for`, `last_fired_at`, and `skipped_total`.

Do not store SaaS tokens or Premium credentials in the scheduler payload,
returned output, KV state, or event-log facts.

## Host proof

The public host runner is `terrane_host::scheduler::run_due` /
`run_due_at`. It records `scheduler.fired`, invokes the app backend through the
app runtime, and logs run errors only at the host edge. Clock ticks are host
input; replay only folds recorded facts.
