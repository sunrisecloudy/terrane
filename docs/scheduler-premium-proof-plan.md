# Scheduler Premium Ops Proof Plan

Public Terrane owns the scheduler capability and host runner. Premium should
consume the public contract after this lands; it should not rely on hidden
Premium-only runtime semantics.

## Public contract dependency

Premium should refresh its pinned Terrane contract and verify that the contract
contains:

- capability namespace `scheduler`
- `ctx.resource.scheduler.create(id, cron, timezone, action, payload)`
- `ctx.resource.scheduler.list()`
- `ctx.resource.scheduler.pause(id)`
- `ctx.resource.scheduler.resume(id)`
- `ctx.resource.scheduler.remove(id)`
- `ctx.resource.scheduler.history(id, limit)`

The grant required for the ops app is the public `scheduler` namespace grant.

## Premium ops app behavior

The Premium-owned ops/admin app should declare `scheduler` in its app manifest
resources and schedule the proof action from its backend:

```js
await ctx.resource.scheduler.create(
  "quickjs-ops-heartbeat",
  "* * * * *",
  "Asia/Bangkok",
  "opsHeartbeat",
  { source: "premium-ops-proof" }
);
```

The action receives the payload as one JSON argument:

```js
var actions = {
  opsHeartbeat: {
    run: function (args) {
      var payload = JSON.parse(args[0]);
      return JSON.stringify({
        ok: true,
        runtime: "quickjs",
        source: payload.source
      });
    }
  }
};
```

## Ops-admin display

The Premium ops-admin surface should show:

- QuickJS runtime activity from normal app invocation telemetry or run output.
- Scheduler definitions from `ctx.resource.scheduler.list()`.
- Run history from `ctx.resource.scheduler.history("quickjs-ops-heartbeat", "50")`.
- Success/failure status, run id, action, started/finished times, output, and error JSON.

Do not store SaaS tokens or Premium credentials in the scheduler payload,
returned output, KV state, or event-log facts.

## Host proof

The public host runner is `terrane_host::scheduler::run_due` /
`run_due_at`. It records `scheduler.run.started`, invokes the app action through
the app runtime, then records `scheduler.run.completed` or
`scheduler.run.failed`. Clock ticks are host input; replay only folds the
recorded facts.
