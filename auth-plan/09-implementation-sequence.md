# Implementation Sequence

## Phase 0 - Keep The Old Plan As History

Do not delete `auth-impl-plan.md`. It records the earlier namespace-only v1. This
folder is the newer plan.

## Phase 1 - Auth Interface Contracts

Add capability-interface types for grantable resources:

```text
GrantResourceSpec
resource selector schema id
supported verbs
optional selector validation helper
```

Update capability docs/export so hosts and admin UIs can discover grantable
resources.

Validation:

- registered resource capabilities expose selector specs;
- docs/contract export includes grantable resources;
- unknown selector schemas are rejected in tests.

## Phase 2 - Reserved KV Auth Storage

Implement `terrane-cap-auth` as a policy facade over reserved KV.

Commands:

```text
auth.member.ensure-local-owner
auth.agent.register
auth.agent.revoke
auth.agent.delegate
auth.permission.request
auth.grant
auth.revoke
```

Queries/helpers:

```text
auth.grantsForRuntime(org, subject, app)
auth.pendingForApp(org, app)
auth.membersForOrg(org)
auth.agentsForOrg(org)
```

If generic `QueryValue` cannot return lists yet, either add `StringList` /
structured JSON values or use internal helper APIs until the query bus expands.

Validation:

- local owner is seeded idempotently;
- grants write reserved KV records;
- public KV cannot read/write auth records;
- grant/revoke are idempotent;
- app removal cleans central auth keys and app projections.

## Phase 3 - Local Admin Control APIs

Add trusted host APIs for local admin UI:

```text
list apps and requested resources
list pending permissions
grant/revoke resource
register/revoke local AI agent
set agent delegation
lock/logout local admin session
```

Do not expose `ctx.resource.auth` to generated apps.

Validation:

- normal app cannot call auth;
- admin API requires active local admin session;
- admin mutations produce policy records/audit lines.

## Phase 4 - Permission Request Broker

Add a shared broker in `terrane-host`.

The broker creates permission requests, stores them through auth/reserved KV,
chooses a trusted UI surface, and returns a request id/URL/resume shape.

Adapters:

```text
host/cli
host/mcp
host/web
host/macos
  -> terrane-host permission broker
```

Do not make `host/cli` or `host/mcp` call `host/web` for auth decisions. Web is
the default dev UI adapter, not the auth backend.

Validation:

- CLI creates a request and opens/prints the web admin URL in dev;
- CLI non-interactive mode exits with a clear permission-required status;
- MCP returns structured `permission_required` data instead of blocking forever;
- mac mode can route to a native request id without changing auth storage;
- approval creates normal grant records and audit lines;
- denial leaves runtime access absent.

## Phase 5 - Local Admin UI

Add host-owned UI at:

```text
/__terrane/admin
```

Initial screens:

- Apps;
- Permission Requests;
- Grants;
- AI Agents;
- Audit.

Validation:

- user can approve a generated app's requested namespace;
- user can revoke and the next run narrows;
- UI can register an AI agent and grant it an app resource.

## Phase 6 - Runtime Namespace Gate

Gate `ctx.resource` namespace installation using:

```text
effective(org, subject, app)
  = manifest.resources(app)
  intersect granted_namespaces(org, subject, app)
```

Cover installed runtime, web invoke, macOS/FFI invoke, preview, and harness JS.

Validation:

- ungranted resource namespace is absent;
- granted namespace is present;
- top-level owner CLI commands remain unaffected;
- replay does not rerun auth or JS.

## Phase 7 - Default Deny

Turn generated apps to default-deny.

Keep dev/test hatch:

```text
TERRANE_DEV_ALLOW_REQUESTED_RESOURCES=1
```

Validation:

- harness-generated apps receive zero grants by default;
- auth tests seed explicit grants;
- existing checked-in examples are migrated or run under dev hatch in tests.

## Phase 8 - Preview And App Builder Parity

Preview uses temporary grants scoped to preview ID.

Validation:

- generated preview gets no resources by default;
- temporary grant enables preview resource;
- destroying preview clears temporary grants;
- installing app requires installed-app grant or promotion flow.

## Phase 9 - Selector-Level Enforcement

After namespace gate is stable, enforce selector/verb checks in
`read_resource`/`write_resource`.

Examples:

```text
relational_db table customers read/write
net host api.example.com fetch
model provider/model call
kv keyPrefix settings/ read
```

Validation:

- namespace grant alone does not over-approve detailed selector once detailed
  mode is enabled;
- denied selector produces clear runtime error;
- capability-owned selector validators are tested in cap crates.

## Phase 10 - Premium Governance Spec

Do not fit this into stale Premium code. Write/replace Premium specs around:

- real accounts and orgs;
- memberships and roles;
- AI agent registry/delegation;
- app permission requests;
- resource grants;
- signed policy snapshots;
- admin UI and audit;
- device authorization;
- generated-app SaaS token boundary.

Validation:

- Premium admin can grant an org AI agent a specific app/resource;
- signed policy snapshot imports into local reserved KV;
- revoked device cannot refresh policy;
- generated app cannot access Premium tokens or APIs.

## Acceptance Criteria For Local V1

- Local owner exists without SaaS login.
- Local admin can grant and revoke app resources.
- Local admin can register and constrain an AI agent.
- Grants are stored under reserved KV and hidden from public app KV.
- Runtime installs only granted namespaces.
- Revocation narrows next run.
- CLI/MCP permission requests route through the shared broker.
- Replay folds recorded facts without reauthorizing.
- App removal cleans auth projections.
- Preview/harness-generated JS follow the same gate.
