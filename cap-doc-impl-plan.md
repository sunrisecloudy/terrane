# Terrane Capability Documentation Exposure Plan

Status: planning artifact only. No Rust implementation has been made in this
worktree.

Thread/worktree name: `cap-doc`.

## Current repo facts verified before writing

- Shared capability ABI lives in `rust/crates/terrane-cap-interface`.
- The live `Capability` trait in
  `rust/crates/terrane-cap-interface/src/lib.rs` currently exposes
  `namespace()`, `manifest()`, `decide()`, `fold()`, `describe()`, `query()`,
  `read_resource()`, and `resource_api()`.
- `CapManifest` currently contains commands, events, queries, backend resource
  methods, and subscriptions. `ResourceMethod` currently contains only method
  name, params, and read/write kind.
- Capability registration and registry-derived surfaces live in
  `rust/crates/terrane-core/src/lib.rs`.
- The default registry currently registers `app`, `build`, `builder`,
  `harness`, `kv`, `crdt`, `replica`, `net`, `model`, and the `host` capability.
- `terrane-core` already generates `docs/APP_API.md`'s `ctx.resource` section
  from capability declarations via `resource_api_markdown()`, and tests guard it
  in `rust/crates/terrane-core/tests/cap/host.rs`.
- Host API wire types and MCP tool descriptors live in
  `rust/crates/terrane-api/src/lib.rs`. This crate intentionally has no
  dependency on `terrane-core`.
- Shared MCP request handling lives in `rust/crates/terrane-host/src/mcp.rs`.
  Stdio MCP (`host/mcp`) and HTTP MCP (`host/web`, `POST /mcp`) both use it.
- CLI command routing lives in `rust/crates/terrane-host/src/cli.rs`; the
  `host/cli` binary is a thin wrapper.
- Public contract export is assembled in `rust/crates/terrane-host/src/lib.rs`
  via `contract_surface()`, typed by `terrane-api`, then wrapped by
  `tools/export-public-contract.mjs`.
- Public contract docs live in `docs/SERVER_API.md`; app/backend docs live in
  `docs/APP_API.md`.
- `rust/crates/terrane-cap-relational-db` does not exist in this checkout yet.
  This plan treats it as a planned crate backed by `terrane-cap-kv`.

## Goal

Expose capability documentation through one canonical interface that can render
as:

- MCP detail: `capabilities_list` and `capability_info`.
- CLI helpers: `terrane cap list`, `terrane cap info <namespace> --format
  json|markdown|skill`, and optional skill export.
- Generated Codex skill/docs: `SKILL.md`, schema references, and examples.
- Public contract export and public docs.

The key product rule is that MCP, CLI, skill, and public docs must not have
separate hand-written capability descriptions that can drift. Every rendered
view should come from the same `CapabilityDoc` tree.

## Architecture

Add a canonical `CapabilityDoc` model to `terrane-cap-interface`, expose it from
the `Capability` trait, derive default docs from `manifest()`, let detailed
capabilities override it, and have `terrane-core` collect docs from the
registered capability instances.

`terrane-api` should contain only the public wire shapes and tool descriptors.
It should not call into the registry directly. `terrane-host` should bridge from
`terrane-core::capability_docs(include_internal)` into the `terrane-api` public
types and into MCP/CLI renderers.

High-level flow:

```text
Capability::doc(include_internal)
  -> terrane-core registry collection
  -> terrane-host render/serve/export helpers
  -> MCP tools, CLI, generated skill, public-contract.json, docs
```

## Canonical data model

Add these structures to `rust/crates/terrane-cap-interface/src/lib.rs`. Use
owned `String` and `Vec` fields rather than `&'static str` so defaults can be
generated from `manifest()` while detailed capabilities can still use static
constants internally.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDoc {
    pub namespace: String,
    pub title: String,
    pub summary: String,
    pub status: CapabilityDocStatus,
    pub version: String,
    pub audience: Vec<DocAudience>,
    pub manifest: CapabilityManifestDoc,
    pub resources: Vec<ResourceDoc>,
    pub commands: Vec<CommandDoc>,
    pub queries: Vec<QueryDoc>,
    pub events: Vec<EventDoc>,
    pub schemas: Vec<SchemaDoc>,
    pub examples: Vec<ExampleDoc>,
    pub constraints: Vec<String>,
    pub limits: Vec<LimitDoc>,
    pub compatibility: Vec<String>,
    pub internal: Vec<InternalNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityDocStatus {
    Stable,
    Experimental,
    Planned,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocAudience {
    AppAuthor,
    Agent,
    HostImplementer,
    InternalMaintainer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityManifestDoc {
    pub commands: Vec<String>,
    pub queries: Vec<String>,
    pub events: Vec<String>,
    pub subscriptions: Vec<String>,
    pub resource_methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMethodDoc {
    pub name: String,
    pub kind: ResourceMethodKind,
    pub params: Vec<ParamDoc>,
    pub returns: Option<ReturnDoc>,
    pub summary: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMethodKind {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDoc {
    pub namespace: String,
    pub summary: String,
    pub methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDoc {
    pub name: String,
    pub summary: String,
    pub args: Vec<ParamDoc>,
    pub records: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDoc {
    pub name: String,
    pub summary: String,
    pub args: Vec<ParamDoc>,
    pub returns: ReturnDoc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDoc {
    pub kind: String,
    pub summary: String,
    pub payload_schema_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDoc {
    pub id: String,
    pub title: String,
    pub media_type: String,
    pub schema_json: String,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleDoc {
    pub title: String,
    pub summary: String,
    pub language: String,
    pub code: String,
    pub expected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitDoc {
    pub name: String,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalNote {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDoc {
    pub name: String,
    pub summary: String,
    pub required: bool,
    pub schema_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnDoc {
    pub summary: String,
    pub schema_ref: Option<String>,
}
```

Serialization boundary:

- Keep `CapabilityDoc` in `terrane-cap-interface` free of `nanoserde` if that is
  preferred for ABI cleanliness, and add matching `terrane-api` wire structs.
- Or add `nanoserde` as a workspace dependency of `terrane-cap-interface` and
  derive `SerJson`/`DeJson` directly. If choosing this route, check crate
  dependency policy first because the interface crate currently uses only
  `borsh` plus std.

Recommended path: keep ABI structs plain and map them to serializable wire
types in `terrane-host`/`terrane-api`, matching the existing pattern where
`terrane-api` owns public JSON and `terrane-core` owns live declarations.

## Trait extension

Extend the trait in `rust/crates/terrane-cap-interface/src/lib.rs`:

```rust
pub trait Capability {
    fn namespace(&self) -> &'static str;

    fn manifest(&self) -> CapManifest {
        CapManifest::empty()
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc {
        CapabilityDoc::from_manifest(self.namespace(), self.manifest(), include_internal)
    }

    // existing decide/fold/describe/query/read_resource/resource_api...
}
```

Default behavior:

- `CapabilityDoc::from_manifest(...)` creates a minimal document using
  `namespace` as title, `manifest.commands`, `manifest.queries`,
  `manifest.events`, `manifest.subscriptions`, and `manifest.resources`.
- Default summaries should be terse and mechanical:
  `Capability namespace '<ns>'`, `Command '<name>'`, `Resource read '<name>'`.
- The default should never include internal notes.
- `include_internal=false` by default everywhere exposed to agents and app
  authors.

Detailed capabilities can override `doc()`. `relational_db` should override it
because its contract is far richer than `manifest().resources` can express.

## Core collection API

Add to `rust/crates/terrane-core/src/lib.rs`:

```rust
pub fn capability_docs(include_internal: bool) -> Vec<CapabilityDoc> {
    default_registry()
        .caps
        .values()
        .map(|c| c.doc(include_internal))
        .collect()
}

pub fn capability_doc(namespace: &str, include_internal: bool) -> Result<CapabilityDoc> {
    let registry = default_registry();
    let cap = registry.get(namespace)?;
    Ok(cap.doc(include_internal))
}
```

Also add markdown and skill renderers in `terrane-core` or `terrane-host`.
Recommendation:

- Put collection in `terrane-core`.
- Put presentation renderers in `terrane-host/src/cap_doc.rs` so core stays
  mostly declaration/runtime oriented and `terrane-host` owns edge formatting.

## Public JSON shapes

Add public wire structs to `rust/crates/terrane-api/src/lib.rs`.

List shape returned by MCP `capabilities_list`, CLI JSON list, and
`public-contract.json.surface.capability_docs_summary`:

```json
{
  "capabilities": [
    {
      "namespace": "kv",
      "title": "Key/value store",
      "summary": "Per-app string key/value storage.",
      "status": "stable",
      "resources": ["kv"],
      "resourceMethods": [
        { "name": "set", "kind": "write" },
        { "name": "get", "kind": "read" }
      ],
      "commands": ["kv.set", "kv.rm", "kv.delete"],
      "events": ["kv.set", "kv.deleted"]
    }
  ]
}
```

Detail shape returned by MCP `capability_info`, CLI
`terrane cap info <namespace> --format json`, and
`public-contract.json.surface.capability_docs`:

```json
{
  "namespace": "relational_db",
  "title": "Relational DB",
  "summary": "Typed table storage and structured queries for app-owned data.",
  "status": "experimental",
  "version": "0.1.0",
  "audience": ["app-author", "agent", "host-implementer"],
  "manifest": {
    "commands": [
      "relational_db.tableCreate",
      "relational_db.tableDrop",
      "relational_db.rowInsert",
      "relational_db.rowPatch",
      "relational_db.rowDelete",
      "relational_db.query"
    ],
    "queries": [],
    "events": [
      "relational_db.table.created",
      "relational_db.row.upserted",
      "relational_db.row.deleted"
    ],
    "subscriptions": ["app.removed"],
    "resourceMethods": [
      {
        "name": "defineTable",
        "kind": "write",
        "params": [
          {
            "name": "table",
            "required": true,
            "summary": "Stable table name.",
            "schemaRef": "table_name.schema.json"
          },
          {
            "name": "spec",
            "required": true,
            "summary": "Table schema JSON.",
            "schemaRef": "table_spec.schema.json"
          }
        ],
        "returns": {
          "summary": "Empty string on success.",
          "schemaRef": null
        },
        "summary": "Create or evolve a table definition.",
        "errors": ["invalid table spec", "unsupported migration"]
      }
    ]
  },
  "resources": [
    {
      "namespace": "relational_db",
      "summary": "App-scoped relational tables backed by kv records.",
      "methods": []
    }
  ],
  "schemas": [
    {
      "id": "table_spec.schema.json",
      "title": "TableSpec",
      "mediaType": "application/schema+json",
      "public": true,
      "schema": {}
    }
  ],
  "examples": [
    {
      "title": "Create and query tasks",
      "language": "js",
      "code": "ctx.resource.relational_db.defineTable(...)",
      "expected": "[{\"id\":\"task_1\"}]"
    }
  ],
  "constraints": [
    "Raw SQL is not exposed.",
    "Tables are app-scoped.",
    "All writes are recorded as deterministic events."
  ],
  "limits": [
    {
      "name": "maxTablesPerApp",
      "value": "64",
      "reason": "Keeps generated apps within local-first storage bounds."
    }
  ]
}
```

When `includeInternal=true`, append:

```json
{
  "internal": [
    {
      "title": "Reserved kv layout",
      "body": "Reserved keys use __rdb/<app>/<table>/<row-id> and __rdb_schema/<app>/<table>."
    }
  ]
}
```

When `includeInternal=false`, omit `internal` entirely or return an empty array.
Prefer omitting it in JSON rendered for MCP/CLI by default to make leakage
harder to miss in tests.

## MCP surface

Add constants and tool descriptors in `rust/crates/terrane-api/src/lib.rs`:

```rust
pub const TOOL_CAPABILITIES_LIST: &str = "capabilities_list";
pub const TOOL_CAPABILITY_INFO: &str = "capability_info";
```

Tool schemas:

```json
{
  "name": "capabilities_list",
  "description": "List Terrane capability namespaces and short summaries.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "includeInternal": {
        "type": "boolean",
        "description": "Include internal-only capability notes. Defaults to false."
      }
    },
    "additionalProperties": false
  }
}
```

```json
{
  "name": "capability_info",
  "description": "Return detailed Terrane capability documentation for one namespace.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "namespace": {
        "type": "string",
        "description": "Capability namespace, e.g. kv, crdt, relational_db."
      },
      "format": {
        "type": "string",
        "enum": ["json", "markdown", "skill"],
        "description": "Rendered output format. Defaults to json."
      },
      "includeInternal": {
        "type": "boolean",
        "description": "Include internal-only implementation notes. Defaults to false."
      }
    },
    "required": ["namespace"],
    "additionalProperties": false
  }
}
```

Implementation in `rust/crates/terrane-host/src/mcp.rs`:

- Extend `CallArgs` with:
  - `namespace: String`
  - `format: String`
  - `include_internal: bool` with `#[nserde(rename = "includeInternal")]`
    if nanoserde supports the rename in the current version; otherwise parse
    with a small dedicated struct or keep a snake-case fallback.
- Add match arms:
  - `TOOL_CAPABILITIES_LIST` -> `capabilities_list_json(include_internal)`.
  - `TOOL_CAPABILITY_INFO` -> `capability_info_render(namespace, format,
    include_internal)`.
- Return tool errors as MCP tool results with `isError: true`, matching existing
  behavior for `app_actions` and `invoke`.
- Preserve existing tools and order. Recommended tool order:
  `list_apps`, `app_actions`, `invoke`, `capabilities_list`,
  `capability_info`.

MCP acceptance:

- `tools/list` advertises both new tools on stdio and HTTP.
- `capability_info` with unknown namespace returns `isError: true` and a clear
  message.
- Default `capability_info` for `relational_db` does not include reserved KV
  layout.
- `capability_info` with `includeInternal=true` includes reserved KV layout.

## CLI surface

Add routing in `rust/crates/terrane-host/src/cli.rs`:

```text
terrane cap list
terrane cap list --format json|markdown
terrane cap info <namespace>
terrane cap info <namespace> --format json|markdown|skill
terrane cap info <namespace> --include-internal
terrane cap export-skill <namespace> --out <dir>
```

Minimum requested commands:

- `terrane cap list`
- `terrane cap info <namespace> --format json|markdown|skill`

Recommended parser behavior:

- Default list format: concise markdown table.
- Default info format: markdown.
- `--format json`: serialize the canonical wire shape.
- `--format markdown`: render human docs.
- `--format skill`: render a complete Codex `SKILL.md` body to stdout.
- `--include-internal`: false by default; opt-in only.
- `terrane cap export-skill <namespace> --out <dir>` writes:
  - `<dir>/SKILL.md`
  - `<dir>/schemas/*.schema.json`
  - `<dir>/examples/*`

Do not add separate hand-written CLI help text per capability. The CLI should
list available namespaces and summaries from `CapabilityDoc`.

## Generated skill artifact

Skill render target for one capability:

```text
<skill-dir>/
  SKILL.md
  schemas/
    table_spec.schema.json
    query.schema.json
    row.schema.json
  examples/
    tasks.js
    tasks.expected.json
```

`SKILL.md` generated sections:

````markdown
# relational_db

Use this skill when building Terrane apps that need typed tables, row writes,
or structured queries through `ctx.resource.relational_db`.

## Contract

- Namespace: `relational_db`
- Status: experimental
- App resource: `ctx.resource.relational_db`

## Methods

| Method | Kind | Summary |
| --- | --- | --- |
| `defineTable(table, spec)` | write | Create or evolve a table definition. |

## Schemas

- `schemas/table_spec.schema.json`
- `schemas/query.schema.json`

## Examples

```js
ctx.resource.relational_db.defineTable("tasks", JSON.stringify({
  primaryKey: "id",
  fields: {
    id: { type: "string", required: true },
    title: { type: "string", required: true },
    done: { type: "boolean", required: true, default: false }
  }
}));
```
````

Rendering rules:

- Skill text is generated from `CapabilityDoc`.
- Schema files come from `CapabilityDoc.schemas`.
- Examples come from `CapabilityDoc.examples`.
- Internal notes are excluded unless an explicit internal export flag is passed.

## Public contract export

Extend `terrane-api::PublicSurface` in `rust/crates/terrane-api/src/lib.rs`:

```rust
pub struct PublicSurface {
    pub contract_version: String,
    pub host: HostContract,
    pub capabilities: Vec<String>,
    pub resources: Vec<ResourceNamespace>,
    pub capability_docs: Vec<CapabilityDocPublic>,
    pub app: AppContractInfo,
    pub sync: SyncInfo,
}
```

Compatibility option:

- Keep `capabilities: Vec<String>` and `resources: Vec<ResourceNamespace>` for
  existing consumers.
- Add `capability_docs` as an additive field.
- Bump `CONTRACT_VERSION` because the public surface shape changes.

Update `rust/crates/terrane-host/src/lib.rs::contract_surface()` to map
`terrane_core::capability_docs(false)` into `CapabilityDocPublic`.

Update `tools/export-public-contract.mjs`:

- Add these contract-defining files to `CONTRACT_FILES`:
  - `cap-doc-impl-plan.md` only while planning is active, or skip it if plans
    should not define public contract hashes.
  - `rust/crates/terrane-cap-interface/src/lib.rs`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-host/src/mcp.rs`
  - `rust/crates/terrane-host/src/cli.rs`
- Keep `docs/SERVER_API.md`, `docs/APP_API.md`, and
  `rust/crates/terrane-api/src/lib.rs`.

Recommended final contract files once implemented:

```js
const CONTRACT_FILES = [
  "docs/SERVER_API.md",
  "docs/APP_API.md",
  "rust/crates/terrane-api/src/lib.rs",
  "rust/crates/terrane-cap-interface/src/lib.rs",
  "rust/crates/terrane-core/src/lib.rs",
  "rust/crates/terrane-host/src/mcp.rs",
  "rust/crates/terrane-host/src/cli.rs",
];
```

## Docs updates

`docs/SERVER_API.md`:

- Add MCP tools `capabilities_list` and `capability_info`.
- Document default redaction: `includeInternal=false`.
- Document the intended discovery order:
  - app workflow: `list_apps` -> `app_actions` -> `invoke`
  - capability workflow: `capabilities_list` -> `capability_info`
- Add public contract note that `capability_docs` is generated from live
  capability declarations.

`docs/APP_API.md`:

- Replace the narrow resource table generator with a richer generated capability
  resource section, or add a second generated section:
  `<!-- generated:capability-docs:start -->`.
- Keep the current `ctx.resource` method table as a compatibility-friendly
  quick reference if useful, but generate it from `CapabilityDoc.resources`
  rather than from a separate path.
- Add `relational_db` docs once the capability exists, including schema refs,
  examples, constraints, and limits.

## `relational_db` capability documentation

Planned crate: `rust/crates/terrane-cap-relational-db`.

Planned dependency: `terrane-cap-kv` for storage backing. The public docs should
describe relational behavior, not the internal KV layout.

Namespace:

```text
relational_db
```

Resource methods:

```text
ctx.resource.relational_db.defineTable(table, specJson)        write
ctx.resource.relational_db.dropTable(table)                    write
ctx.resource.relational_db.insert(table, rowJson)              write
ctx.resource.relational_db.patch(table, id, patchJson)         write
ctx.resource.relational_db.delete(table, id)                   write
ctx.resource.relational_db.get(table, id)                      read
ctx.resource.relational_db.query(table, queryJson)             read
ctx.resource.relational_db.tables()                           read
ctx.resource.relational_db.describeTable(table)                read
```

Commands:

```text
relational_db.tableCreate
relational_db.tableDrop
relational_db.rowInsert
relational_db.rowPatch
relational_db.rowDelete
relational_db.query
```

Events:

```text
relational_db.table.created
relational_db.table.dropped
relational_db.row.upserted
relational_db.row.deleted
```

Schemas to embed in `CapabilityDoc.schemas`:

- `table_name.schema.json`
- `table_spec.schema.json`
- `row.schema.json`
- `patch.schema.json`
- `query.schema.json`
- `query_result.schema.json`

`table_spec.schema.json` should include:

```json
{
  "$id": "table_spec.schema.json",
  "type": "object",
  "required": ["primaryKey", "fields"],
  "additionalProperties": false,
  "properties": {
    "primaryKey": {
      "type": "string",
      "pattern": "^[A-Za-z_][A-Za-z0-9_]*$"
    },
    "fields": {
      "type": "object",
      "minProperties": 1,
      "additionalProperties": {
        "type": "object",
        "required": ["type"],
        "additionalProperties": false,
        "properties": {
          "type": {
            "type": "string",
            "enum": ["string", "number", "boolean", "json"]
          },
          "required": { "type": "boolean" },
          "default": {},
          "indexed": { "type": "boolean" },
          "unique": { "type": "boolean" },
          "deprecated": { "type": "boolean" }
        }
      }
    },
    "indexes": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["name", "fields"],
        "additionalProperties": false,
        "properties": {
          "name": {
            "type": "string",
            "pattern": "^[A-Za-z_][A-Za-z0-9_]*$"
          },
          "fields": {
            "type": "array",
            "items": { "type": "string" },
            "minItems": 1
          },
          "unique": { "type": "boolean" }
        }
      }
    }
  }
}
```

`query.schema.json` should include:

```json
{
  "$id": "query.schema.json",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "where": { "$ref": "#/$defs/filter" },
    "orderBy": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["field"],
        "additionalProperties": false,
        "properties": {
          "field": { "type": "string" },
          "direction": { "type": "string", "enum": ["asc", "desc"] }
        }
      }
    },
    "limit": { "type": "integer", "minimum": 0, "maximum": 1000 },
    "offset": { "type": "integer", "minimum": 0 }
  },
  "$defs": {
    "filter": {
      "oneOf": [
        {
          "type": "object",
          "required": ["field", "op", "value"],
          "additionalProperties": false,
          "properties": {
            "field": { "type": "string" },
            "op": {
              "type": "string",
              "enum": ["eq", "ne", "lt", "lte", "gt", "gte", "in", "contains"]
            },
            "value": {}
          }
        },
        {
          "type": "object",
          "required": ["and"],
          "additionalProperties": false,
          "properties": {
            "and": {
              "type": "array",
              "items": { "$ref": "#/$defs/filter" },
              "minItems": 1
            }
          }
        },
        {
          "type": "object",
          "required": ["or"],
          "additionalProperties": false,
          "properties": {
            "or": {
              "type": "array",
              "items": { "$ref": "#/$defs/filter" },
              "minItems": 1
            }
          }
        }
      ]
    }
  }
}
```

Examples:

- Create a `tasks` table.
- Insert a task.
- Patch `done`.
- Query incomplete tasks ordered by priority.
- Show error for raw SQL or unknown field.

Constraints:

- Raw SQL is never exposed.
- Table and field names must be portable identifiers.
- App data is scoped by app id.
- Writes are deterministic recorded events.
- Reads are derived from folded state and are not recorded.
- Migrations are limited to additive fields, deprecating fields, and adding
  indexes unless a later migration engine is explicitly designed.
- Query results must be stable ordered when `orderBy` is present; without
  `orderBy`, return primary-key order.

Limits:

- `maxTablesPerApp`: 64.
- `maxFieldsPerTable`: 128.
- `maxIndexesPerTable`: 16.
- `maxQueryLimit`: 1000.
- `maxRowBytes`: 65536.
- `maxSpecBytes`: 65536.

Internal notes hidden unless `includeInternal=true`:

- Reserved KV schema key layout.
- Reserved row key layout.
- Encoding/version prefix.
- Rebuild/migration strategy.
- Any temporary compatibility aliases used during migration.

Suggested internal note text:

```text
Reserved KV layout: table specs are stored under a reserved app-private schema
prefix and rows under a reserved row prefix. These keys are implementation
details and must not be generated into app-facing docs unless
includeInternal=true.
```

## File-by-file implementation plan

`rust/crates/terrane-cap-interface/src/lib.rs`

- Add `CapabilityDoc` and child data structs.
- Add `Capability::doc(include_internal)` default.
- Add `CapabilityDoc::from_manifest(...)`.
- Add tests in `rust/crates/terrane-cap-interface/src/tests.rs` for default doc
  generation from a manifest.

`rust/crates/terrane-core/src/lib.rs`

- Re-export `CapabilityDoc` types if needed.
- Add `capability_docs(include_internal)` and
  `capability_doc(namespace, include_internal)`.
- Add a generated markdown helper only if presentation stays in core.
- Add tests in `rust/crates/terrane-core/tests/cap/interface.rs` or a new
  `rust/crates/terrane-core/tests/cap/docs.rs`.

`rust/crates/terrane-api/src/lib.rs`

- Add public wire structs for capability doc summary/detail.
- Add MCP tool constants and descriptors.
- Add `capability_docs` to `PublicSurface`.
- Bump `CONTRACT_VERSION`.
- Update `rust/crates/terrane-api/tests/contract.rs` to assert the tool set and
  JSON round-trips.

`rust/crates/terrane-host/src/lib.rs`

- Map `terrane_core::capability_docs(false)` into public API wire types for
  `contract_surface()`.
- Add helper functions:
  - `capabilities_list(include_internal: bool) -> CapabilityList`
  - `capability_info(namespace, include_internal) -> CapabilityDocPublic`

`rust/crates/terrane-host/src/cap_doc.rs`

- New module for rendering:
  - JSON through `nanoserde::SerJson`.
  - Markdown.
  - Skill `SKILL.md`.
  - Skill export file list.
- Keep format handling here so MCP and CLI share it.

`rust/crates/terrane-host/src/mcp.rs`

- Advertise new tools from `terrane_api::mcp_tools()`.
- Handle `capabilities_list` and `capability_info`.
- Add tests in `rust/crates/terrane-host/src/mcp_tests.rs`.

`rust/crates/terrane-host/src/cli.rs`

- Add `cap` command routing.
- Add `--format` and `--include-internal` parsing.
- Update help text to list `terrane cap list` and `terrane cap info`.

`host/cli/src/main.rs`

- No direct implementation needed unless wrapper-specific help changes are
  desired; it delegates to `terrane_host::cli::run`.

`host/mcp/tests/mcp.rs`

- Update tool list assertion from three tools to include new capability tools.
- Add stdio MCP call tests for `capabilities_list` and `capability_info`.

`host/web/tests/web.rs`

- Update HTTP MCP tool list assertions.
- Add HTTP MCP call tests for `capabilities_list` and `capability_info`.

`docs/SERVER_API.md`

- Document the two capability MCP tools.
- Document `includeInternal` behavior.

`docs/APP_API.md`

- Update generated resource docs to consume the new capability doc source.
- Add relational DB app-facing examples once the crate exists.

`tools/export-public-contract.mjs`

- Add contract-defining source files as described above.
- Confirm exported JSON includes `surface.capability_docs`.

`tools/verify-public-contract.mjs`

- Existing surface equality and file hash verification should keep working; add
  any extra schema validation only if needed.

`rust/Cargo.toml`

- When implementing `relational_db`, add
  `crates/terrane-cap-relational-db` to workspace members and dependencies.

`rust/crates/terrane-core/Cargo.toml`

- Add dependency on `terrane-cap-relational-db` when the capability exists.

`rust/crates/terrane-core/src/lib.rs`

- Add `RelationalDbState` to `State`.
- Add `StateStore` get/get_mut arms for `relational_db`.
- Register `RelationalDbCapability` in `default_registry()`.

## Tests

Unit tests:

- `terrane-cap-interface`: default `CapabilityDoc` generated from `CapManifest`.
- `terrane-cap-interface`: `include_internal=false` strips internal notes.
- `terrane-cap-interface`: manifest-derived resource docs preserve read/write
  kind and parameter order.
- `terrane-cap-relational-db`: detailed `doc(false)` contains public schemas,
  examples, constraints, and limits.
- `terrane-cap-relational-db`: `doc(false)` omits reserved KV layout.
- `terrane-cap-relational-db`: `doc(true)` includes reserved KV layout.

Core tests:

- Registry docs include every registered namespace.
- `capability_doc("kv", false)` succeeds.
- Unknown namespace returns the same clear error style as current registry lookup.
- `capability_docs(false)` is sorted/stable if the registry iteration order is
  not already stable enough for contract exports.

API tests:

- `mcp_tools()` includes `capabilities_list` and `capability_info`.
- New tool input schemas parse as JSON objects.
- Public capability doc wire types round-trip with `nanoserde`.
- `host_contract()` lists the new tools.

Host tests:

- `terrane_host::contract_surface()` includes `capability_docs`.
- `surface.capabilities` remains for compatibility.
- `surface.resources` remains for compatibility.
- JSON export for `relational_db` excludes internals by default.

MCP tests:

- `rust/crates/terrane-host/src/mcp_tests.rs`: direct handler tests for both
  tools.
- `host/mcp/tests/mcp.rs`: stdio `tools/list` advertises both tools and
  `tools/call` returns capability data.
- `host/web/tests/web.rs`: HTTP `/mcp` path advertises and calls both tools.

CLI tests:

- Add tests under `host/cli/tests` or `rust/crates/terrane-host` for:
  - `terrane cap list`
  - `terrane cap list --format json`
  - `terrane cap info kv --format markdown`
  - `terrane cap info kv --format json`
  - `terrane cap info relational_db --format skill`
  - unknown namespace
  - invalid format

Docs/contract tests:

- Existing `docs/APP_API.md` generated section test should be updated to use the
  new source.
- Add a generated docs test for capability docs if a new marker is introduced.
- `tools/verify-public-contract.mjs` passes after export.

## Validation commands

Run after implementation:

```sh
cd rust && cargo test -p terrane-cap-interface --locked
cd rust && cargo test -p terrane-core --test cap --locked
cd rust && cargo test -p terrane-api --locked
cd rust && cargo test -p terrane-host --locked
cd host/cli && cargo test --locked
cd host/mcp && cargo test --locked
cd host/web && cargo test --locked
cd rust && cargo clippy --workspace --all-targets --locked -- -D warnings
UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc
node --no-warnings tools/export-public-contract.mjs --out /tmp/terrane-public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract /tmp/terrane-public-contract.json
git diff --check
```

If host/web or network/socket tests fail with `Operation not permitted`, rerun
the same already-built tests outside the sandbox before classifying it as a
source regression. This matches the known local sandbox behavior for Terrane
host tests.

Manual smoke examples:

```sh
cd rust
cargo run -q -p terrane-host --bin terrane -- cap list
cargo run -q -p terrane-host --bin terrane -- cap info kv --format json
cargo run -q -p terrane-host --bin terrane -- cap info relational_db --format skill
cargo run -q -p terrane-host --bin terrane -- cap info relational_db --format json --include-internal
```

MCP smoke:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"capabilities_list","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"capability_info","arguments":{"namespace":"relational_db","format":"json"}}}
```

## Migration and compatibility notes

- Add `Capability::doc()` with a default implementation so all existing
  capabilities compile without immediate overrides.
- Keep `manifest()` as the registry validation source for commands, queries,
  events, resources, and subscriptions. `doc()` enriches the surface; it should
  not replace manifest validation in the first implementation.
- Keep `resource_api()` working exactly as today for the QuickJS runtime
  injection path in `rust/crates/terrane-core/src/host_runtime.rs`.
- Derive default `doc().resources` from `resource_api()` so existing
  `ctx.resource` docs remain correct.
- Keep `PublicSurface.capabilities` and `PublicSurface.resources` as additive
  compatibility fields even after adding `capability_docs`.
- Bump `terrane-api::CONTRACT_VERSION` because public contract JSON changes.
- Existing MCP tools (`list_apps`, `app_actions`, `invoke`) keep their names and
  behavior.
- Existing CLI generic command routing (`<namespace> <verb> [args...]`) remains.
  `cap` becomes a reserved top-level CLI command; if a future capability wants
  namespace `cap`, reject that namespace explicitly.
- `includeInternal=false` is the default in MCP, CLI, public contract export,
  and generated skill output.
- Internal implementation notes should never be included in
  `public-contract.json` unless the export command grows an explicit internal
  mode. The public contract should remain app-author/agent safe.
- `relational_db` may use `terrane-cap-kv` internally, but its public docs should
  not require app authors to know the KV key layout.
- If `relational_db` ships before the full engine is done, mark its doc status
  `Planned` or `Experimental` and keep commands/resources absent until runtime
  support exists. Do not advertise callable resource methods before the runtime
  actually injects them.

## Acceptance criteria

- A single `CapabilityDoc` source produces MCP, CLI, generated skill, docs, and
  public contract views.
- Every registered capability appears in `terrane cap list`,
  `capabilities_list`, and `public-contract.json.surface.capability_docs`.
- `capability_info` and `terrane cap info` can render `json`, `markdown`, and
  `skill`.
- `docs/APP_API.md` generated capability/resource content is derived from
  `CapabilityDoc` or the same underlying declarations, with tests preventing
  drift.
- `docs/SERVER_API.md` documents the capability MCP tools.
- Existing app MCP workflow remains unchanged.
- Existing public `capabilities` and `resources` fields remain available for
  consumers.
- `relational_db` docs include full schema docs, query schema, method docs,
  examples, constraints, and limits.
- `relational_db` reserved KV layout appears only with `includeInternal=true`.
- Contract export and verifier pass.
- No implementation-only internal notes are exposed to agents/app authors by
  default.
