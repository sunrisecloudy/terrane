# Auth Plan

This folder is the current auth planning pack. It supersedes the older
namespace-only grant shape in `auth-impl-plan.md` without deleting that document.

The new model keeps local Terrane useful without an account, while making room
for organization-owned Premium policy later.

## Documents

- `01-decisions.md` - settled decisions and corrections to the old plan.
- `02-identity-subjects.md` - orgs, users, AI agents, anonymous subjects.
- `03-login-logout-sessions.md` - login, logout, local owner, SaaS sessions.
- `04-membership-roles.md` - org membership, roles, and agent delegation.
- `05-resource-grants.md` - grant schema and capability-owned selectors.
- `06-storage-reserved-kv.md` - reserved KV layout for local policy records.
- `07-runtime-gate-admin.md` - runtime gate, local admin UI, preview parity.
- `08-premium-governance.md` - Terrane Premium target spec, assuming old Premium
  docs/code are stale.
- `09-implementation-sequence.md` - build order and acceptance criteria.
- `10-login-logout-ui.md` - host-owned admin UI, session state, login/logout UX.
- `11-permission-request-broker.md` - shared CLI/MCP/web/mac permission request
  broker and UI routing.
- `12-review-followup-decisions.md` - locked follow-up decisions after the first
  external review, before code implementation.
- `13-storage-target.md` - storage target for event log and projections, split
  from auth correctness.
- `14-grant-resource-spec-code-plan.md` - code-facing plan for mandatory grant
  resource specs across `terrane-cap-*`.

## Core Shape

The grant question is:

```text
May this subject run this app with access to this resource inside this org?
```

So the canonical policy key is:

```text
(org, subject, app, resource_selector)
```

For local v1 this collapses to:

```text
org     = local
subject = user:local-owner
app     = installed/generated app id
```

The product split is:

```text
Public/local Terrane
  local runtime gate
  reserved-KV auth records
  cap-owned resource selectors
  local admin UI
  local-owner and local-agent grants

Terrane Premium
  real accounts and organizations
  org memberships and roles
  AI agent delegation
  app permission request workflow
  org policy sync to devices
  audit, billing, support, marketplace governance
```

Generated apps must never receive SaaS tokens or admin authority. They only see
the `ctx.resource.*` surface the runtime installs for them.
