# Identity And Subjects

## Two Axes

The six subject classes come from two axes:

```text
actor kind: user | ai_agent
identity:   anonymous | me/local_owner | other_user
```

Resulting groups:

```text
anonymous + ai_agent -> anonymous ai agent
me        + ai_agent -> my ai agent
other     + ai_agent -> other user ai agent

anonymous + user     -> anonymous user
me        + user     -> me as user
other     + user     -> other user
```

The schema should not hardcode six unrelated roles. It should represent actor
kind and identity class.

## Subject IDs

Suggested canonical subject identifiers:

```text
user:anonymous
user:local-owner
user:<premium-user-id>

agent:anonymous:<agent-id>
agent:local-owner:<agent-id>
agent:<premium-user-id>:<agent-id>
```

The string ID is for keys and logs. The structured subject object is for API
payloads and policy evaluation.

```json
{
  "kind": "ai_agent",
  "identity": "local_owner",
  "id": "codex-local",
  "ownerUser": "user:local-owner"
}
```

## Organizations

Users belong to organizations. Organizations are the tenant boundary.

```text
organization
  users
  ai agents
  apps
  memberships
  roles
  grants
  policy snapshots
```

Local Terrane seeds a fake organization:

```text
org = local
```

Premium replaces this with real organization IDs.

## Subject Authority

Human user authority comes from membership, role, and resource grants.

AI agent authority is clamped:

```text
agent authority =
  owner user authority
  intersect agent delegation
  intersect app/resource grants
  intersect org policy
  intersect device/session policy when present
```

An agent should normally not be able to grant itself new permissions.

## Anonymous Subjects

Anonymous subjects exist for public demos, shared links, marketplace browsing,
and unauthenticated local preview. They get no membership by default.

Anonymous access must be explicit public policy later, not an implicit fallback.

Local v1 may simply treat anonymous subjects as denied for protected resources.
