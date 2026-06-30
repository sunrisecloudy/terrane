# Premium Auth And Agent Governance

## Status

Existing `terrane-premium` docs and code are stale relative to this auth plan.
Treat them as old context, not target authority.

This document defines the new Premium target around the current Terrane auth
direction.

## Product Split

```text
Local Terrane
  lets me grant my local AI agent

Terrane Premium
  lets an organization govern which users and agents may use which apps and
  resources, then syncs signed policy down to authorized devices
```

Generated apps are not Premium clients. They do not get Premium tokens.

## Premium Responsibilities

- real accounts;
- organizations and memberships;
- AI agent registry;
- agent delegation;
- permission request workflow;
- resource grants;
- signed policy snapshots;
- device authorization;
- audit;
- billing/entitlement gates;
- admin/support/operator controls;
- marketplace and team catalog governance.

## Premium Domain Entities

```text
users
  id
  email
  display_name
  status

organizations
  id
  name
  status

memberships
  id
  org_id
  subject_id
  subject_kind
  role
  status

ai_agents
  id
  org_id
  owner_user_id
  display_name
  provider
  status

agent_delegations
  id
  org_id
  agent_id
  owner_user_id
  max_role
  can_install_apps
  can_request_permissions
  can_grant_permissions
  expires_at
  status

app_permission_requests
  id
  org_id
  workspace_id
  app_id
  requested_by_subject
  requested_resources_json
  status
  reviewed_by
  reviewed_at

resource_grants
  id
  org_id
  subject_id
  app_id
  resource_id
  resource_selector_json
  verbs_json
  source
  status

policy_snapshots
  id
  org_id
  device_id
  version
  signed_policy_json
  valid_until
```

## Authority Rule

```text
agent authority =
  owner user membership
  intersect agent delegation
  intersect app/resource grant
  intersect org policy
  intersect device authorization
  intersect entitlement state
```

Example:

```text
Alice may use Codex Agent in org Acme.
Codex Agent may help with CRM app.
Codex Agent may read/write relational_db table customers.
Codex Agent may not grant itself new permissions.
```

## Premium APIs

Platform-owned clients and admin UI call these APIs. Generated apps do not.

```text
GET  /orgs/:orgId/agents
POST /orgs/:orgId/agents
POST /orgs/:orgId/agents/:agentId/revoke

GET  /orgs/:orgId/agent-delegations
POST /orgs/:orgId/agent-delegations
PATCH /orgs/:orgId/agent-delegations/:delegationId

GET  /orgs/:orgId/permission-requests
POST /orgs/:orgId/permission-requests/:requestId/approve
POST /orgs/:orgId/permission-requests/:requestId/reject

GET  /orgs/:orgId/resource-grants
POST /orgs/:orgId/resource-grants
DELETE /orgs/:orgId/resource-grants/:grantId

GET  /orgs/:orgId/policy-snapshot
POST /orgs/:orgId/policy-snapshot/refresh
```

## Premium Admin UI

Required views:

- members and roles;
- AI agents;
- agent delegations;
- apps;
- permission requests;
- resource grants;
- policy snapshots;
- devices;
- audit log;
- entitlements affecting policy.

## Policy Snapshot Sync

Premium policy reaches local Terrane through signed snapshots.

```text
1. Premium admin changes policy.
2. Premium writes audit events.
3. Premium generates signed policy snapshot.
4. Authorized platform client downloads snapshot.
5. Local Terrane verifies signature and imports non-secret policy records into
   reserved KV.
6. Runtime gate consumes local reserved KV only.
```

If offline, local Terrane may use a still-valid cached policy snapshot. Expired
snapshots should fail closed for cloud-managed grants unless org policy says
otherwise.

## Premium Login/Logout

Premium login creates a user/device session and fetches org policy. Premium
logout revokes/clears SaaS tokens and stops policy refresh/sync.

Local generated apps continue to have no SaaS token access in both states.

## Audit Requirements

Audit these mutations:

- login/logout/session revoke;
- device register/revoke;
- member invite/remove/role change;
- AI agent register/revoke;
- agent delegation create/update/revoke;
- permission request approve/reject;
- resource grant/revoke;
- policy snapshot issue/import;
- admin export;
- operator/support access.

Audit records are append-only. Corrections are new records.
