# Runtime Gate And Admin UI

Detailed login/logout and admin workspace UI is expanded in
`10-login-logout-ui.md`. CLI/MCP/web/mac permission-request routing is expanded
in `11-permission-request-broker.md`.

## Runtime Gate

The runtime gate computes:

```text
effective(org, subject, app)
  = manifest.resources(app)
  intersect granted_namespaces(org, subject, app)
```

For local v1:

```text
org = local
subject = user:local-owner
```

For agent execution:

```text
subject = agent:<owner-user>:<agent-id>
```

## Gate Placement

The gate must live in the shared runtime resource path, not only in CLI or host
UI code.

It must cover:

- installed app backend runs;
- web host invokes;
- macOS/FFI invokes;
- App Builder preview;
- harness-generated JS runs.

Namespace-level v1 can filter at resource installation:

```text
RuntimeResourceHost.resource_methods(namespace)
  -> return resource_api only if grant exists
```

Future selector-level enforcement belongs in:

```text
read_resource(namespace, method, args)
write_resource(namespace, method, args)
```

## Denial UX

Denied namespace:

```text
ctx.resource.<namespace> is absent
```

This matches current undeclared-resource behavior and avoids inventing a new JS
error path.

Future selector-level denial can return a runtime error naming the blocked
method and selector without leaking sensitive data.

## Local Admin UI

Local admin is the first control plane UX. It should be host-owned/trusted.

Suggested route:

```text
/__terrane/admin
```

It is not a generated app with `ctx.resource.auth`. It calls protected host
control APIs that dispatch auth commands as the active admin subject.

Required local admin views:

- installed apps;
- pending app permissions;
- granted resources;
- AI agents;
- agent delegation;
- local membership/role state;
- audit/history;
- preview permission prompts.

The request creation path should be shared through `terrane-host`; `host/cli`
and `host/mcp` should not call `host/web` to make auth decisions.

## Admin Actions

```text
grant resource
revoke resource
register local AI agent
revoke local AI agent
set agent delegation
approve permission request
reject permission request
lock/logout local admin session
```

## Preview And App Builder

Preview runs generated code and must be default-deny too.

Flow:

```text
1. App Builder generates files.
2. Preview parses manifest.resources.
3. Admin UI shows pending resources.
4. User grants temporary preview permissions or denies them.
5. Preview runtime uses the same gate.
6. If app is installed, user can convert temporary grants to installed-app grants.
```

Temporary preview grants should be scoped to the preview ID and cleared when the
preview is destroyed unless explicitly promoted.

## Dev Escape Hatch

Keep a dev/test hatch:

```text
TERRANE_DEV_ALLOW_REQUESTED_RESOURCES=1
```

Rules:

- only dev/test;
- not used by auth tests;
- visible in logs/admin UI;
- should not be enabled in production host builds.
