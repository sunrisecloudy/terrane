# Auth Decisions

## What Changed From `auth-impl-plan.md`

The old implementation plan used:

```text
grant key = (app, resource_namespace)
```

That was good for the first generated-app confinement discussion, but it is too
small once Terrane includes users, organizations, AI agents, anonymous subjects,
and detailed resource selectors.

The new key is:

```text
grant key = (org, subject, app, resource_selector)
```

The runtime can still enforce namespace-level v1 grants first, but the stored
shape must not require a migration when finer selectors arrive.

## Settled Decisions

1. Auth v1 remains a user-to-app confinement slice. It closes the "request is
   grant" hole where generated app code declares its own resources and receives
   them automatically.

2. The confined execution principal is not only the app. It is:

   ```text
   org + subject + app
   ```

   Local v1 seeds `org:local` and `user:local-owner`.

3. Subjects include humans and AI agents. An AI agent is not equal to its owner.
   Its effective authority is delegated and clamped.

4. Users belong to organizations. Organizations are the tenant boundary for
   Premium and the natural future boundary for sync, marketplace, policy, and
   audit.

5. Auth records should live in platform-owned reserved KV, not public app KV.
   Public `ctx.resource.kv` must continue to reject and hide `__terrane/` keys.

6. Resource details belong to the target capability crate. Auth stores generic
   grant envelopes and asks capability-owned contracts what selectors and verbs
   mean.

7. The runtime gate should be in the shared runtime resource path, not only in a
   host CLI path. Installed app runs, preview runs, and harness-generated JS need
   the same rule.

8. The first runtime gate can be namespace-level:

   ```text
   requested manifest namespace + any matching grant -> install ctx.resource.ns
   ```

   Later method-level checks can enforce table, host, model, path, or document
   selectors.

9. The admin UI is a trusted host/control-plane surface. It is not a normal app
   with `ctx.resource.auth`.

10. Premium should be treated as a new target spec. Existing Premium docs and
    code are stale context, not authority.

## Non-Goals For Local V1

- No full user account system is required to run local apps.
- No SaaS login is required for local generation, install, run, storage, or
  inspection.
- No generated app receives SaaS sessions, admin tokens, billing tokens, sync
  tokens, signing keys, or direct Premium API access.
- No MAC markings, lineage propagation, or per-marking encryption in this slice.

## Replay Rule

Authorization checks happen before intent becomes fact. Replay folds recorded
facts and does not re-authorize old facts.

```text
live run:  check policy -> run JS once -> record resource writes
replay:    fold recorded writes only
```

Grant and revoke records are replayable policy facts. The runtime gate itself is
not replayed.
