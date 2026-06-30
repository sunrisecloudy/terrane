# Reserved KV Storage

## Direction

Auth records are platform-owned data stored on top of reserved KV. Public app KV
must continue to reject and hide `__terrane/` keys.

Auth owns the commands and policy meaning. KV owns the durable substrate.

## Local KV App Scope

Use a dedicated internal app scope for auth:

```text
app = __terrane/auth
```

All keys under this app are platform-owned. They are not installed into
`ctx.resource.kv` for generated apps.

## Canonical Grant Key

Use subject-first canonical keys:

```text
__terrane/auth/v1/orgs/<org>/grants/subjects/<subject>/apps/<app>/resources/<resource_id>
```

Example:

```text
app   = __terrane/auth
key   = __terrane/auth/v1/orgs/local/grants/subjects/user:local-owner/apps/crm/resources/relational_db/table/customers
value = {
  "org": "local",
  "subject": "user:local-owner",
  "app": "crm",
  "resource": {
    "namespace": "relational_db",
    "selector": { "kind": "table", "name": "customers" },
    "verbs": ["read", "write"]
  },
  "source": "local_admin",
  "status": "active"
}
```

## App Lookup Projection

Runtime needs quick "what is granted for this app?" lookup. Add an app-indexed
projection:

```text
__terrane/auth/v1/orgs/<org>/grants_by_app/apps/<app>/subjects/<subject>/resources/<resource_id>
```

The value may duplicate the grant JSON or point to the canonical key. Duplication
is simpler for v1; pointer reduces update payload later.

## Membership Keys

```text
__terrane/auth/v1/orgs/<org>/members/users/<user>
__terrane/auth/v1/orgs/<org>/members/agents/<agent>
__terrane/auth/v1/orgs/<org>/role_bindings/<subject>/<scope>
__terrane/auth/v1/orgs/<org>/agent_delegations/<agent>
```

Local v1 only needs:

```text
orgs/local/members/users/user:local-owner
```

Agent grants can be added as soon as local admin can register an AI agent.

## Permission Requests

```text
__terrane/auth/v1/orgs/<org>/permission_requests/<request-id>
__terrane/auth/v1/orgs/<org>/permission_requests_by_app/<app>/<request-id>
```

Permission requests are admin workflow records, not runtime grants.

## Policy Snapshots

Premium will sync signed policy snapshots down to devices.

```text
__terrane/auth/v1/orgs/<org>/policy_snapshots/<snapshot-id>
__terrane/auth/v1/orgs/<org>/active_policy_snapshot
```

Local-only v1 does not need signatures. Premium snapshots should be signed by the
platform and imported by platform-owned client code, not generated apps.

## App Removal Cleanup

If grants live under `__terrane/auth`, app removal does not clean them for free.
The auth capability must react to `app.removed` and delete:

```text
.../grants/subjects/*/apps/<app>/...
.../grants_by_app/apps/<app>/...
.../permission_requests_by_app/<app>/...
```

Cleanup is deterministic because it emits/deletes platform KV records as part of
the command path, and replay folds the resulting facts.

## Event Shape

Even if stored as KV records, auth should expose conceptual policy events:

```text
auth.granted
auth.revoked
auth.permission.requested
auth.permission.approved
auth.permission.rejected
auth.member.added
auth.role.assigned
auth.agent.delegated
```

Implementation can map these to `kv.set` / `kv.deleted` records under reserved
keys first, then add explicit event records if audit/search requires it.
