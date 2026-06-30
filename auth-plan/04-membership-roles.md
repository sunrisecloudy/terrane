# Membership, Roles, And Delegation

## Membership

Membership answers:

```text
Can this subject act inside this organization?
```

Resource grants answer:

```text
Can this subject run this app with this resource?
```

Keep them separate.

## Membership Record

```json
{
  "org": "local",
  "subject": { "kind": "user", "id": "user:local-owner" },
  "role": "owner",
  "status": "active"
}
```

Premium uses real org and user IDs.

## Roles

Initial org roles:

```text
owner
  full org control, including owners, admin policy, grants, export

admin
  members, apps, agents, grants, devices, policy; cannot remove final owner

developer
  create/install/update apps, request resources, manage dev workflows

operator
  operate apps and data workflows; limited policy mutation

member
  use approved apps and resources granted to them

viewer
  read-only org/app access

auditor
  read audit/policy/history, no mutation

guest
  limited invited access
```

Premium may keep plan-specific roles later, but the policy engine should map
them to explicit permissions rather than rely on names alone.

## AI Agent Delegation

An AI agent is a subject, but it acts through delegated authority.

```json
{
  "org": "local",
  "agent": "agent:local-owner:codex-local",
  "ownerUser": "user:local-owner",
  "maxRole": "developer",
  "canInstallApps": true,
  "canRequestPermissions": true,
  "canGrantPermissions": false,
  "expiresAt": null,
  "status": "active"
}
```

Effective authority:

```text
agent authority =
  owner user authority
  intersect agent delegation
  intersect app/resource grant
```

## Grant Authority

Suggested defaults:

```text
owner/admin
  may grant/revoke app resources

developer
  may request resources and install apps, but not grant by default

agent
  may request permissions only when delegated; cannot grant by default

anonymous
  cannot grant or request protected permissions by default
```

## App Permission Requests

Permission requests are not grants. They are pending intent.

```json
{
  "org": "local",
  "app": "crm",
  "requestedBy": "agent:local-owner:codex-local",
  "resources": [
    {
      "namespace": "relational_db",
      "selector": { "kind": "table", "name": "customers" },
      "verbs": ["read", "write"]
    }
  ],
  "status": "pending"
}
```

Admin UI approves/rejects requests. Runtime consumes grants only.
