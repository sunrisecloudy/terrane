# Grant Resource Spec Code Plan

This document plans the codebase changes needed to make grant resource specs
mandatory for every runtime resource exposed by `terrane-cap-*`.

It is a planning artifact only. No code implementation has been started here.

## Goal

Make this invariant true in code:

```text
No capability can expose ctx.resource.<namespace> unless it declares a
GrantResourceSpec for that namespace.

No generated app can request a manifest resource unless that namespace is backed
by a registered GrantResourceSpec.
```

The resource spec catalog becomes the shared source for:

- runtime resource installation;
- auth grant validation;
- permission request UI;
- App Builder manifest validation;
- MCP/CLI capability docs;
- public contract export.

## Current Runtime Resource Surfaces

These crates currently expose `ctx.resource.*` methods and therefore need specs
in the first code pass:

```text
terrane-cap-kv
  ctx.resource.kv.set
  ctx.resource.kv.get
  ctx.resource.kv.all
  ctx.resource.kv.rm
  ctx.resource.kv.scan
  ctx.resource.kv.range
  ctx.resource.kv.keys

terrane-cap-crdt
  ctx.resource.crdt.mapSet
  ctx.resource.crdt.mapGet
  ctx.resource.crdt.mapAll
  ctx.resource.crdt.mapDel
  ctx.resource.crdt.listPush
  ctx.resource.crdt.listInsert
  ctx.resource.crdt.listDel
  ctx.resource.crdt.listAll
  ctx.resource.crdt.textInsert
  ctx.resource.crdt.textDel
  ctx.resource.crdt.textGet

terrane-cap-relational-db
  ctx.resource.relational_db.defineTable
  ctx.resource.relational_db.put
  ctx.resource.relational_db.delete
  ctx.resource.relational_db.get
  ctx.resource.relational_db.query
  ctx.resource.relational_db.tables
  ctx.resource.relational_db.spec

terrane-cap-build
  ctx.resource.build.compileTs
```

These crates currently do not expose runtime resources and do not need specs in
the first pass:

```text
terrane-cap-app
terrane-cap-builder
terrane-cap-harness
terrane-cap-js-runtime
terrane-cap-model
terrane-cap-net
terrane-cap-replica
terrane-cap-wasm-runtime
```

If `model` or `net` later exposes `ctx.resource.model` or `ctx.resource.net`,
the same commit must add grant resource specs for those namespaces.

The planned `document` resource should not stay in generated-app allow-lists
until a real registered document capability exposes resource methods and specs.

## Interface Direction

Add static grant specs to `CapManifest`, beside commands, events, queries,
resources, and subscriptions.

Planning shape:

```rust
pub struct CapManifest {
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub queries: Vec<QuerySpec>,
    pub resources: Vec<ResourceMethod>,
    pub grant_resources: Vec<GrantResourceSpec>,
    pub subscriptions: Vec<EventPattern>,
}

pub struct GrantResourceSpec {
    pub namespace: &'static str,
    pub selector_schema_id: &'static str,
    pub selector_title: &'static str,
    pub selector_summary: &'static str,
    pub verbs: &'static [&'static str],
    pub compatibility: GrantResourceCompatibility,
}

pub struct GrantResourceCompatibility {
    pub compatible_with: &'static [&'static str],
    pub unknown_schema_policy: UnknownSelectorSchemaPolicy,
}

pub enum UnknownSelectorSchemaPolicy {
    Deny,
}
```

`GrantResourceSpec` belongs in `CapManifest` because the spec catalog is static
metadata. The registry can validate it before runtime or auth logic runs.

Selector behavior still belongs to the capability that owns the namespace:

```text
validate_selector(schema_id, selector_json)
selector_id(schema_id, selector_json)
selector_summary(schema_id, selector_json)
```

That behavior can be added as a capability method or a helper trait in a later
implementation pass. The static catalog still lives in `CapManifest`.

## Required Registry Validation

`Registry::validate()` should enforce:

- if `manifest.resources` is non-empty, `manifest.grant_resources` is non-empty;
- every `GrantResourceSpec.namespace` equals the capability namespace;
- every runtime-resource capability declares a `namespace.v1` spec;
- selector schema ids are unique per namespace;
- every spec has at least one verb;
- unknown selector schema policy is fail-closed;
- no generated/runtime resource namespace appears in allow-lists unless it has a
  registered spec.

This makes missing specs fail at startup/test time instead of silently exposing a
resource.

## Built-In Namespace Spec

Every runtime resource capability gets a built-in namespace selector:

```text
selector_schema_id = namespace.v1
selector           = { "kind": "namespace" }
selector_id        = <namespace>
unknown schema     = deny
```

This avoids a separate namespace-only code path.

The first runtime gate can grant `namespace.v1` while future selector-level
gates add more specific schemas such as `key-prefix.v1` or `table.v1`.

## Capability Spec Inventory

## terrane-cap-kv

Immediate spec:

```text
namespace = kv
selector_schema_id = namespace.v1
selector_id = kv
verbs = read, list, write, delete
```

Methods covered:

```text
read/list:
  get
  all
  scan
  range
  keys

write/delete:
  set
  rm
```

Future selector specs:

```text
key-prefix.v1
  selector = { "prefix": "settings/" }
  selector_id = prefix:<escaped-prefix>
  verbs = read, list, write, delete

key.v1
  selector = { "key": "settings/theme" }
  selector_id = key:<escaped-key>
  verbs = read, write, delete
```

Compatibility requirement:

```text
namespace.v1 must remain readable forever.
key-prefix.v1 and key.v1 must fail closed on unknown future versions.
```

Reserved-key rule:

```text
Public ctx.resource.kv still rejects and hides __terrane/ keys even if kv is
granted.
```

## terrane-cap-crdt

Immediate spec:

```text
namespace = crdt
selector_schema_id = namespace.v1
selector_id = crdt
verbs = read, write
```

Methods covered:

```text
read:
  mapGet
  mapAll
  listAll
  textGet

write:
  mapSet
  mapDel
  listPush
  listInsert
  listDel
  textInsert
  textDel
```

Future selector specs:

```text
doc.v1
  selector = { "doc": "profile" }
  selector_id = doc:<escaped-doc>
  verbs = read, write

container.v1
  selector = { "doc": "profile", "kind": "map|list|text" }
  selector_id = container:<escaped-doc>:<kind>
  verbs = read, write
```

Compatibility requirement:

```text
Old CRDT selector schemas must keep their old validator/id behavior, or be
migrated through recorded auth facts.
```

## terrane-cap-relational-db

Immediate spec:

```text
namespace = relational_db
selector_schema_id = namespace.v1
selector_id = relational_db
verbs = schema, read, query, write, delete
```

Methods covered:

```text
schema:
  defineTable
  spec
  tables

read/query:
  get
  query

write/delete:
  put
  delete
```

Future selector specs:

```text
table.v1
  selector = { "table": "customers" }
  selector_id = table:<escaped-table>
  verbs = schema, read, query, write, delete

index.v1
  selector = { "table": "customers", "index": "byEmail" }
  selector_id = index:<escaped-table>:<escaped-index>
  verbs = query
```

Compatibility requirement:

```text
table.v1 must remain valid for old grants even if the relational table spec
format evolves.
Breaking selector meaning changes require table.v2 or index.v2.
```

## terrane-cap-build

Immediate spec:

```text
namespace = build
selector_schema_id = namespace.v1
selector_id = build
verbs = compile
```

Methods covered:

```text
compile:
  compileTs
```

This resource is pure and sandboxed, but it still exposes compute to generated
code. It therefore needs a grant spec and must be default-deny like other
resources.

Policy note:

```text
App Builder or harness previews may request temporary build grants.
Installed apps should not receive build grants unless explicitly approved.
```

## Future terrane-cap-net

No current `ctx.resource.net` surface exists.

When added, likely specs:

```text
origin.v1
  selector = { "origin": "https://api.example.com" }
  selector_id = origin:<escaped-origin>
  verbs = fetch

host.v1
  selector = { "host": "api.example.com" }
  selector_id = host:<escaped-host>
  verbs = fetch
```

Network is high-risk and should not be added as a namespace-only grant unless
the product explicitly wants broad outbound access.

## Future terrane-cap-model

No current `ctx.resource.model` surface exists.

When added, likely specs:

```text
provider-model.v1
  selector = { "provider": "openai", "model": "gpt-5" }
  selector_id = model:<escaped-provider>:<escaped-model>
  verbs = call
```

Model resources need special UI treatment because they may imply cost,
credential, privacy, or Premium entitlement boundaries.

## Planned Document Resource

`document` appears in planning/docs and old builder allow-lists, but it is not a
registered runtime resource capability today.

Rule:

```text
document must be removed from generated-app resource allow-lists until a real
document capability exposes resource methods and GrantResourceSpec metadata.
```

When implemented:

```text
document.v1
  selector = { "document": "<id-or-scope>" }
  selector_id = document:<escaped-id-or-scope>
  verbs = read, write, append
```

## Builder Validation Plan

`terrane-cap-builder` currently validates generated manifests with a hard-coded
resource allow-list.

That should change.

Avoid making `terrane-cap-builder` depend on `terrane-core`; that would create
the wrong direction of dependency.

Recommended code direction:

```text
terrane-cap-builder
  validate_files_with_resources(files, app_id, name, allowed_resources)
  validate_files(files, app_id, name) only handles structural validation or uses
  a narrow test-only default

terrane-core / terrane-host edge
  computes allowed_resources from default_registry().grant_resource_namespaces()
  passes allowed_resources into builder validation
```

Acceptance:

```text
Generated manifest resources are accepted only if the namespace has a registered
GrantResourceSpec.
```

## Runtime Installation Plan

Before full auth exists, add a spec gate:

```text
RuntimeResourceHost.resource_methods(namespace)
  -> return no methods if namespace has no GrantResourceSpec
```

After auth exists, add the real grant gate:

```text
RuntimeResourceHost.resource_methods(namespace)
  -> if no GrantResourceSpec: deny
  -> if no runtime grant for ExecutionPrincipal + app + namespace: deny
  -> else return resource methods
```

Selector-level enforcement later lives in:

```text
read_resource(namespace, method, args)
write_resource(namespace, method, args)
```

## Docs And Contract Plan

Expose grant resource specs in:

- capability docs;
- MCP `capability_info`;
- CLI `cap info`;
- public contract export;
- admin UI permission editor.

Planning API additions:

```text
CapabilityManifestInfo.grant_resources
CapabilityResourceInfo.grant_specs
PublicSurface.resources[].grant_specs
```

Docs should show both:

```text
ctx.resource methods
grant selector schemas
```

## Test Plan

Add tests before default-deny rollout:

- registry validation fails when a capability exposes `resources` without
  `grant_resources`;
- `kv`, `crdt`, `relational_db`, and `build` declare `namespace.v1`;
- generated app resource validation derives from grant specs, not a manual list;
- `document` is rejected until a real registered document resource exists;
- runtime host does not install a resource namespace without a spec;
- public contract includes grant specs for all runtime resource namespaces;
- unknown future selector schema fails closed;
- old selector schema fixtures stay valid;
- dev hatch, if present, cannot bypass the "spec exists" requirement.

## Implementation Order

1. Add `GrantResourceSpec` metadata types to `terrane-cap-interface`.
2. Add `grant_resources` to `CapManifest`.
3. Add `namespace.v1` specs to `kv`, `crdt`, `relational_db`, and `build`.
4. Add registry validation for resource/spec invariants.
5. Add helper APIs to list grant resource namespaces/specs.
6. Update builder validation to receive allowed resources from core/host.
7. Update runtime resource installation to deny namespaces without specs.
8. Export grant specs through docs/MCP/CLI/public contract.
9. Add detailed selector behavior for `kv`, `crdt`, and `relational_db`.
10. Add selector-level runtime enforcement after namespace gate is stable.

## Non-Goals For This Slice

- Do not implement full auth grants yet.
- Do not implement Premium policy sync yet.
- Do not add `ctx.resource.net` or `ctx.resource.model` in this slice.
- Do not expose `document` until a real resource capability exists.
- Do not make app-visible KV able to read reserved auth or relational keys.

## Open Questions

- Should `GrantResourceSpec.namespace` be stored explicitly, or inferred from the
  owning capability namespace during registry validation?
- Should verbs be method kinds (`read`/`write`) or domain verbs
  (`schema`/`query`/`compile`)? This plan recommends domain verbs.
- Should selector behavior live on `Capability`, on a new `GrantResourceBehavior`
  trait, or on helper functions per crate?
- Should `build` grants be allowed for installed apps, or only preview/harness
  runs by default?
