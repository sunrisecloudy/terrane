# Resource Grants And Selectors

## Grant Envelope

Auth owns the generic grant envelope:

```json
{
  "org": "local",
  "subject": "user:local-owner",
  "app": "crm",
  "resource": {
    "namespace": "relational_db",
    "selector": { "kind": "table", "name": "customers" },
    "verbs": ["read", "write"]
  }
}
```

Auth does not own the meaning of `table`, `host`, `model`, `document`, or
`keyPrefix`. The target capability owns that.

## Capability-Owned Selector Contract

Add a capability-interface contract similar to:

```rust
pub struct GrantResourceSpec {
    pub namespace: &'static str,
    pub selector_schema_id: &'static str,
    pub verbs: &'static [&'static str],
}
```

Each capability declares valid selector shapes and verbs:

```text
kv
  selector v1: { "kind": "namespace" }
  later:       { "kind": "keyPrefix", "prefix": "settings/" }
  verbs:       read, write

crdt
  selector v1: { "kind": "namespace" }
  later:       { "kind": "document", "name": "todos" }
  verbs:       read, write

relational_db
  selector v1: { "kind": "namespace" }
  later:       { "kind": "table", "name": "customers" }
  verbs:       read, write

net
  selector:    { "kind": "host", "host": "api.example.com" }
  verbs:       fetch

model
  selector:    { "kind": "model", "provider": "openai", "model": "gpt-5" }
  verbs:       call
```

## V1 Namespace Collapse

For local v1, the runtime gate can collapse detailed grants to namespaces:

```text
if any grant has namespace relational_db
then install ctx.resource.relational_db
```

This keeps the first implementation small.

Later, `read_resource` and `write_resource` can enforce selectors and verbs from
method arguments:

```text
ctx.resource.relational_db.put("customers", row)
  -> requires relational_db table customers write

ctx.resource.net.fetch("https://api.example.com/users")
  -> requires net host api.example.com fetch
```

## Manifest Requests

Current app manifests request only namespaces:

```json
{
  "resources": ["kv", "relational_db"]
}
```

Keep that for v1.

Later optional detailed requests can be added:

```json
{
  "resources": [
    {
      "namespace": "relational_db",
      "selector": { "kind": "table", "name": "customers" },
      "verbs": ["read", "write"]
    }
  ]
}
```

Detailed requests improve prompting and review. They should not weaken runtime
enforcement.

## Grant Lifecycle

```text
app declares/request resources
admin UI shows requested - granted
admin approves selector/verbs
auth stores grant
runtime installs namespace-level surface in v1
future resource methods enforce selector-level policy
```

## Idempotency

Grant ID should be canonical:

```text
resource_id = <namespace>/<selector-id>
```

Granting the same `(org, subject, app, resource_id)` twice is idempotent.

Revoking a missing grant is idempotent and leaves the post-state "not granted".
