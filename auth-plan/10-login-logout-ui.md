# Login, Logout, And Admin UI Plan

This document expands `03-login-logout-sessions.md` into product UI behavior.
Shared permission-request routing for CLI, MCP, web, and macOS is expanded in
`11-permission-request-broker.md`.

The UI rule is:

```text
Terrane login changes who can administer policy.
It must not be required for ordinary local app usage.
```

## UI Goals

- Make the active authority visible: local, Premium, locked, offline, or expired.
- Keep local Terrane useful before any SaaS account exists.
- Let the owner grant/revoke app and AI-agent access without editing raw KV.
- Make generated-app permission requests reviewable before runtime access exists.
- Never expose Premium tokens, admin tokens, or reserved auth KV to generated apps.

## Trusted Surfaces

Auth UI is host-owned. It is not a generated Terrane app.

Primary route:

```text
/__terrane/admin
```

Related host-owned surfaces:

```text
/__terrane/admin/session
/__terrane/admin/requests
/__terrane/admin/grants
/__terrane/admin/agents
/__terrane/admin/audit
```

The current `host/web` code already reserves `__terrane` for trusted host
routes such as builder and preview. The admin UI should follow that pattern.

## First Screen

The first screen should be the admin workspace, not a landing page.

Layout:

```text
left nav
  Overview
  Requests
  Apps
  Grants
  Agents
  People
  Audit
  Settings

top authority bar
  org selector
  active subject
  authority source
  sync/offline state
  lock/logout action
  Premium sign in/out action

main pane
  focused table or editor for selected nav item
```

The design should feel like an operational tool: dense, calm, readable, and
optimized for repeated review of permissions.

## Authority Bar

The authority bar is the UI anchor for login/logout state.

It displays:

```text
org
subject
source
state
```

Examples:

```text
Local / user:local-owner / local / unlocked
Local / none / local / locked
Acme / user:alice / Premium / online
Acme / user:alice / Premium / offline snapshot valid
Acme / user:alice / Premium / expired
```

Actions:

- Lock local admin.
- Unlock local admin.
- Sign in to Premium.
- Sign out of Premium.
- Switch org.
- Refresh policy snapshot.

The authority bar should be rendered by host UI outside generated app iframes.

## Local First-Run UI

On first local run:

```text
1. Host seeds org:local, user:local-owner, and owner membership.
2. Admin UI opens with local authority unlocked for v1.
3. Authority bar shows Local / local-owner / unlocked.
4. Requests, Apps, Grants, Agents, and Audit are available.
5. Premium sign-in is optional.
```

No account-creation wizard is required for v1 local use.

Later hardening can insert a local unlock sheet before step 3.

## Local Locked UI

When local admin is locked:

```text
1. Installed apps continue with already-granted runtime policy.
2. Admin UI remains visible, but mutation controls are disabled.
3. Permission requests can be viewed.
4. Grant, revoke, role, agent, and org-policy actions require unlock.
```

Allowed while locked:

- view installed apps;
- view current grants;
- view pending requests;
- view audit;
- open app runtime views.

Blocked while locked:

- grant resource;
- revoke resource;
- approve request;
- register or revoke agent;
- assign role;
- change org policy;
- export sensitive audit/policy data.

Unlock v1:

```text
1. User clicks Unlock.
2. Host creates in-memory local_admin_session.
3. Mutation controls become enabled.
4. Audit records local session unlock.
```

Later unlock can require Touch ID, passkey, OS account confirmation, or a local
admin password.

## Local Logout UI

Use the product label `Lock` for local mode. It is clearer than cloud-style
logout because local mode has no SaaS account.

Flow:

```text
1. User clicks Lock.
2. Host drops local_admin_session.
3. UI changes state to Local / none / locked.
4. Current runtime grants remain effective.
5. Admin mutations require Unlock.
```

No app data is deleted. No grants are revoked. No generated app is stopped just
because the admin UI locked.

## Premium Signed-Out UI

Premium sign-in should be available from the authority bar and Settings.

Signed out state:

```text
local org remains usable
Premium policy sync is inactive
Premium org switcher is hidden or disabled
Premium-only admin sections show empty signed-out state
```

The UI should not block local permission setup while signed out.

## Premium Login UI

Flow:

```text
1. User clicks Sign in to Premium.
2. Host opens the platform-owned Premium auth flow.
3. Browser/auth window completes user authentication.
4. Device authorization completes.
5. Host stores Premium tokens in OS credential storage.
6. Admin UI fetches org memberships and entitlements.
7. User chooses active Premium org if multiple orgs exist.
8. Host imports non-secret policy snapshot into reserved KV.
9. Authority bar shows selected org, Premium subject, and sync state.
```

Generated apps never receive the Premium session.

If local grants conflict with Premium policy, the UI should show the conflict in
Grants and require an explicit resolution action.

## Premium Logout UI

Use the product label `Sign out` for Premium mode.

Flow:

```text
1. User clicks Sign out.
2. UI asks whether to keep cached policy for offline use when allowed.
3. Host revokes the Premium session when online.
4. Host clears Premium tokens from OS credential storage.
5. Policy refresh stops.
6. Authority bar returns to Local or Locked.
7. Cloud-managed grants remain usable only if snapshot validity allows it.
```

Premium logout should not delete local app data by default.

## Offline And Expired States

Premium policy snapshots need explicit UI state.

States:

```text
online
offline_snapshot_valid
offline_snapshot_expiring
offline_snapshot_expired
token_expired
sync_error
```

Behavior:

- `offline_snapshot_valid`: allow cloud-managed grants until expiry.
- `offline_snapshot_expiring`: allow grants and show refresh action.
- `offline_snapshot_expired`: treat cloud-managed grants as denied.
- `token_expired`: require Premium sign-in before sync or Premium mutations.
- `sync_error`: keep local admin usable and show retry details.

## Permission Request UI

Permission requests are the core admin workflow.

Request row shape:

```text
request id
app or preview id
app name
request source
resource namespace
resource selector summary
verbs
target subject
status
created at
```

Statuses:

```text
pending
approved temporary
approved installed
denied
revoked
expired
```

Actions:

- approve for preview;
- approve for installed app;
- deny;
- edit selector;
- change target subject;
- expire temporary grant;
- view manifest and capability docs.

For v1, selector editing can be namespace-only. The UI should still reserve
space for capability-owned selector details so relational DB, KV, model, file,
and future capabilities can render different selector editors.

## Grant Editor UI

Grant editor fields:

```text
org
subject
app
resource namespace
resource selector
verbs
source
created by
created at
expires at
status
reason
```

Subject picker groups:

```text
Users
  me
  other users
  anonymous user

AI agents
  my agents
  other user agents
  anonymous agent
```

Local v1 can show only:

```text
me
my local agents
anonymous user
anonymous agent
```

Premium can add org members, teams, roles, and cross-user agents.

## Agent UI

Agent list row shape:

```text
agent id
display name
owner user
org
trust level
delegation summary
last used
status
```

Actions:

- register local agent;
- revoke agent;
- set delegation;
- view grants;
- view audit;
- lock agent from new sessions.

Agent detail should show the clamp:

```text
effective agent authority
  = owner authority
  intersect agent delegation
  intersect app/resource grants
  intersect org/device/session policy
```

## App Detail UI

App detail tabs:

```text
Manifest
Requests
Grants
Subjects
Audit
```

Manifest tab shows requested resources from `manifest.resources`.

Requests tab shows pending and historical permission requests.

Grants tab shows current effective grants grouped by subject.

Subjects tab shows which users and agents can run the app with which resources.

Audit tab shows grant, revoke, preview, and runtime-denial events.

## Preview And App Builder UI

Preview grants are temporary by default.

Flow:

```text
1. App Builder creates a preview.
2. Preview manifest lists requested resources.
3. Requests view shows preview-scoped permission rows.
4. User can approve temporary preview grants.
5. Preview runtime uses the same gate as installed apps.
6. User can promote temporary grants during install.
7. Temporary preview grants expire when the preview is destroyed.
```

The preview frame should never silently get all requested resources.

## Denial UI

Runtime denial should be visible in admin, even if generated app JS only sees a
missing resource namespace.

Admin event shape:

```text
time
org
subject
app
resource namespace
method if known
selector summary if known
decision = denied
reason
```

Reasons:

```text
no grant
grant expired
snapshot expired
subject locked
agent delegation missing
app not installed
capability not available
```

## API Needs

The UI needs protected host control APIs. Names are planning placeholders.

```text
GET  /__terrane/admin/session
POST /__terrane/admin/local/unlock
POST /__terrane/admin/local/lock

POST /__terrane/admin/premium/login/start
POST /__terrane/admin/premium/login/complete
POST /__terrane/admin/premium/logout
POST /__terrane/admin/premium/sync

GET  /__terrane/admin/orgs
GET  /__terrane/admin/requests
POST /__terrane/admin/requests/:id/approve
POST /__terrane/admin/requests/:id/deny

GET  /__terrane/admin/grants
POST /__terrane/admin/grants
DELETE /__terrane/admin/grants/:id

GET  /__terrane/admin/agents
POST /__terrane/admin/agents
DELETE /__terrane/admin/agents/:id

GET  /__terrane/admin/audit
```

These APIs execute as the active host admin subject. Generated apps cannot call
them through `ctx.resource`.

## UI Acceptance Criteria

Local v1:

- A fresh `TERRANE_HOME` opens admin with `org:local` and `user:local-owner`.
- User can lock and unlock local admin without deleting app data.
- Locked admin blocks grant/revoke actions.
- Installed apps keep already-granted runtime resources after local lock.
- App Builder preview requests appear before resources are installed.
- Temporary preview grants do not become installed-app grants unless promoted.
- The UI shows effective grants by subject, app, and resource.

Premium-ready:

- Premium sign-in is optional for local use.
- Premium tokens are stored only in OS credential storage.
- Imported policy snapshots contain no SaaS secrets.
- Premium sign-out clears tokens and stops sync.
- Offline snapshot expiry changes runtime grant decisions.
- Org switch changes the active policy view and effective subject.

## Open Product Decisions

- Whether local v1 starts unlocked or starts with a one-click Unlock action.
- Whether local lock should happen automatically after inactivity.
- Whether Premium logout should keep valid offline policy by default.
- How much policy conflict resolution belongs in local Terrane vs Premium.
- Which capability renders the first selector-specific grant editor.
